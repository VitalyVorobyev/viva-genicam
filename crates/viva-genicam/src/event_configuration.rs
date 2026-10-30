//! Explicit, reversible event notification setup for a single camera session.

use std::net::Ipv4Addr;
use std::sync::{Arc, Weak};

use crate::genapi::{GenApiError, RegisterIo};
use crate::{Camera, ControlState, GenicamError, GigeRegisterIo};
use viva_gige::consts::{MESSAGE_DESTINATION_ADDRESS, MESSAGE_DESTINATION_PORT};
use viva_sfnc::{EVENT_NOTIFICATION, EVENT_SELECTOR, MSG_EN, MSG_IP, MSG_PORT, MSG_SEL};

/// Event settings changed by one successful setup, tied to its camera session.
///
/// Restore explicitly with [`Camera::restore_events`] before releasing the camera.
/// Dropping this token performs no I/O: it may be dropped on an async worker or
/// after another application took control. It must never be replayed onto a
/// replacement camera. This token does not own an event reader; stop and join
/// readers separately, outside any camera/device lock.
#[derive(Debug)]
#[must_use = "restore the changed event settings explicitly before releasing the camera"]
pub struct EventConfiguration {
    identity: Weak<()>,
    permission: Permission,
    event_selector: Option<String>,
    notifications: Vec<(String, String)>,
    channel: Option<Channel>,
}

#[derive(Debug)]
enum Permission {
    Gige(tokio::sync::watch::Receiver<ControlState>),
    #[cfg(feature = "u3v")]
    U3v(crate::U3vConnection),
    #[cfg(test)]
    Memory,
}

impl Permission {
    fn skipped(&self) -> Option<EventRestoreSkipped> {
        match self {
            Self::Gige(control) => match *control.borrow() {
                ControlState::Held => None,
                state => Some(EventRestoreSkipped::NotInControl(state)),
            },
            #[cfg(feature = "u3v")]
            Self::U3v(connection) => connection
                .is_lost()
                .then_some(EventRestoreSkipped::Disconnected),
            #[cfg(test)]
            Self::Memory => None,
        }
    }
}

#[derive(Debug, Default)]
struct Channel {
    raw: Vec<(u64, Vec<u8>)>,
    // name, original selector, selector configured by configure_events
    selector: Option<(String, String, String)>,
    features: Vec<(String, String)>,
}

/// Why restoration deliberately performed no further writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum EventRestoreSkipped {
    /// The token belongs to a different or retired camera session.
    DifferentCamera,
    /// The GigE session no longer holds control.
    NotInControl(ControlState),
    /// The USB transport has reported disconnection.
    Disconnected,
}

/// One setting that could not be restored. Other independent settings are
/// still attempted while the camera remains writable.
#[derive(Debug)]
pub struct EventRestoreFailure {
    /// Feature name, selector context, or bootstrap register address.
    pub setting: String,
    /// The original typed error.
    pub error: GenicamError,
}

/// The outcome of restoring only the settings an event setup changed.
#[derive(Debug, Default)]
pub struct EventRestoreReport {
    /// Individual restoration failures, including failed selector changes.
    pub failures: Vec<EventRestoreFailure>,
    /// The reason remaining restoration was skipped, if any.
    pub skipped: Option<EventRestoreSkipped>,
}

impl EventRestoreReport {
    /// Whether every saved setting was restored or already held its old value.
    pub fn is_complete(&self) -> bool {
        self.failures.is_empty() && self.skipped.is_none()
    }
}

/// Setup failed; [`Self::source`] retains the error and [`Self::restoration`]
/// says whether rollback succeeded. A failed rollback must not be reported as
/// a successful cleanup.
#[derive(Debug, thiserror::Error)]
#[error("event setup failed: {source}; rollback has {} failure(s), skipped: {:?}", .restoration.failures.len(), .restoration.skipped)]
pub struct EventSetupError {
    /// The original setup error.
    #[source]
    pub source: GenicamError,
    /// What happened when settings already changed were rolled back.
    pub restoration: EventRestoreReport,
}

impl<T: RegisterIo> Camera<T> {
    /// Restore event settings from this same camera session, best-effort.
    ///
    /// No writes occur for a different camera, a monitor, lost/released control,
    /// or a disconnected USB transport. The transport also checks ownership
    /// on every write, so a refusal stops later writes as soon as loss is known.
    /// This is blocking register I/O; call on a blocking thread.
    pub fn restore_events(&mut self, saved: EventConfiguration) -> EventRestoreReport {
        let mut report = EventRestoreReport::default();
        if !Weak::ptr_eq(&saved.identity, &Arc::downgrade(&self.event_identity)) {
            report.skipped = Some(EventRestoreSkipped::DifferentCamera);
            return report;
        }
        if let Some(reason) = saved.permission.skipped() {
            report.skipped = Some(reason);
            return report;
        }

        for (selector, previous) in &saved.notifications {
            if !restore_step(&saved.permission, &mut report, EVENT_SELECTOR, || {
                set_if_changed(self, EVENT_SELECTOR, selector)
            }) {
                if report.skipped.is_some() {
                    break;
                }
                // A failed selector write must never restore another selector's value.
                continue;
            }
            restore_step(
                &saved.permission,
                &mut report,
                &format!("{EVENT_NOTIFICATION}[{selector}]"),
                || set_if_changed(self, EVENT_NOTIFICATION, previous),
            );
        }
        if let Some(previous) = &saved.event_selector {
            restore_step(&saved.permission, &mut report, EVENT_SELECTOR, || {
                set_if_changed(self, EVENT_SELECTOR, previous)
            });
        }
        if let Some(channel) = &saved.channel {
            let selected = if let Some((name, _, target)) = &channel.selector {
                restore_step(&saved.permission, &mut report, name, || {
                    set_if_changed(self, name, target)
                })
            } else {
                true
            };
            if selected {
                for (name, previous) in channel.features.iter().rev() {
                    restore_step(&saved.permission, &mut report, name, || {
                        set_if_changed(self, name, previous)
                    });
                }
            }
            // The old helper can fall back after partially setting SFNC nodes.
            // Restore both actual bootstrap registers and those SFNC settings.
            for (address, previous) in channel.raw.iter().rev() {
                restore_step(
                    &saved.permission,
                    &mut report,
                    &format!("0x{address:08x}"),
                    || {
                        if self.transport.read(*address, previous.len())? != *previous {
                            self.transport.write(*address, previous)?;
                        }
                        Ok(())
                    },
                );
            }
            if let Some((name, previous, _)) = &channel.selector {
                restore_step(&saved.permission, &mut report, name, || {
                    set_if_changed(self, name, previous)
                });
            }
        }
        report
    }

    fn snapshot_notifications(
        &mut self,
        selectors: &[&str],
        saved: &mut EventConfiguration,
    ) -> Result<(), GenicamError> {
        // Validate all choices before even temporary selector writes, and
        // before configuring any message channel.
        let entries = self
            .feature_state(EVENT_SELECTOR)?
            .enum_entries
            .unwrap_or_default();
        for &selector in selectors {
            if !entries.iter().any(|entry| entry == selector) {
                return Err(GenApiError::EnumNoSuchEntry {
                    node: EVENT_SELECTOR.into(),
                    entry: selector.into(),
                }
                .into());
            }
        }
        saved.event_selector = Some(self.get(EVENT_SELECTOR)?);
        for &selector in selectors {
            if saved.notifications.iter().any(|(name, _)| name == selector) {
                continue;
            }
            set_if_changed(self, EVENT_SELECTOR, selector)?;
            let notification = self.feature_state(EVENT_NOTIFICATION)?;
            if !notification
                .enum_entries
                .unwrap_or_default()
                .iter()
                .any(|n| n == "On")
            {
                return Err(GenApiError::EnumNoSuchEntry {
                    node: EVENT_NOTIFICATION.into(),
                    entry: "On".into(),
                }
                .into());
            }
            saved
                .notifications
                .push((selector.into(), self.get(EVENT_NOTIFICATION)?));
        }
        if let Some(previous) = &saved.event_selector {
            set_if_changed(self, EVENT_SELECTOR, previous)?;
        }
        Ok(())
    }

    fn snapshot_channel(&mut self, saved: &mut EventConfiguration) -> Result<(), GenicamError> {
        saved.channel = Some(Channel::default());
        let channel = saved.channel.get_or_insert_with(Channel::default);
        for address in [MESSAGE_DESTINATION_ADDRESS, MESSAGE_DESTINATION_PORT] {
            channel
                .raw
                .push((address, self.transport.read(address, 4)?));
        }
        if let Some(name) = self.find_alias(MSG_SEL) {
            let original = self.get(name)?;
            let target = self.enum_entries(name)?.into_iter().next().ok_or_else(|| {
                GenApiError::EnumNoSuchEntry {
                    node: name.into(),
                    entry: "a message channel".into(),
                }
            })?;
            channel.selector = Some((name.into(), original, target.clone()));
            set_if_changed(self, name, &target)?;
        }
        for aliases in [MSG_IP, MSG_PORT, MSG_EN] {
            if let Some(name) = self.find_alias(aliases) {
                channel.features.push((name.into(), self.get(name)?));
            }
        }
        if let Some((name, previous, _)) = &channel.selector {
            set_if_changed(self, name, previous)?;
        }
        Ok(())
    }

    fn setup_error(&mut self, source: GenicamError, saved: EventConfiguration) -> EventSetupError {
        EventSetupError {
            source,
            restoration: self.restore_events(saved),
        }
    }
}

impl Camera<GigeRegisterIo> {
    /// Configure event notification with an explicit restoration token.
    ///
    /// Bind [`Self::open_event_stream`] first (port 0 is supported), then use
    /// its actual port here. Requested event choices are validated before
    /// message-channel writes. A setup error includes rollback results.
    /// No configuration is automatically replayed on a replacement camera.
    pub async fn configure_events_restorable(
        &mut self,
        local_ip: Ipv4Addr,
        port: u16,
        selectors: &[&str],
    ) -> Result<EventConfiguration, EventSetupError> {
        let mut saved = EventConfiguration {
            identity: Arc::downgrade(&self.event_identity),
            permission: Permission::Gige(self.transport.watch_control()),
            event_selector: None,
            notifications: Vec::new(),
            channel: None,
        };
        if let Some(EventRestoreSkipped::NotInControl(state)) = saved.permission.skipped() {
            return Err(EventSetupError {
                source: GenicamError::NotInControl(state),
                restoration: EventRestoreReport::default(),
            });
        }
        let prepared = self
            .snapshot_notifications(selectors, &mut saved)
            .and_then(|()| self.snapshot_channel(&mut saved));
        if let Err(error) = prepared {
            return Err(self.setup_error(error, saved));
        }
        if let Err(error) = self.configure_events(local_ip, port, selectors).await {
            return Err(self.setup_error(error, saved));
        }
        // Selection is context, not part of what the event reader needs.
        if let Err(error) = restore_selectors(self, &saved) {
            return Err(self.setup_error(error, saved));
        }
        Ok(saved)
    }
}

#[cfg(feature = "u3v")]
impl<T: crate::u3v::UsbTransfer + 'static> Camera<crate::U3vRegisterIo<T>> {
    /// Enable named USB3 Vision events, retaining only changed settings for
    /// explicit restoration. Open the event reader separately; it preserves
    /// the EventEnable word it found when opened.
    pub fn configure_events_restorable(
        &mut self,
        selectors: &[&str],
    ) -> Result<EventConfiguration, EventSetupError> {
        let mut saved = EventConfiguration {
            identity: Arc::downgrade(&self.event_identity),
            permission: Permission::U3v(self.transport.connection()),
            event_selector: None,
            notifications: Vec::new(),
            channel: None,
        };
        if self.transport.is_lost() {
            return Err(EventSetupError {
                source: GenicamError::U3v {
                    during: "configure events",
                    source: crate::u3v::U3vError::Disconnected,
                },
                restoration: EventRestoreReport::default(),
            });
        }
        if let Err(error) = self
            .snapshot_notifications(selectors, &mut saved)
            .and_then(|()| self.configure_events(selectors))
            .and_then(|()| restore_selectors(self, &saved))
        {
            return Err(self.setup_error(error, saved));
        }
        Ok(saved)
    }
}

fn restore_selectors<T: RegisterIo>(
    camera: &mut Camera<T>,
    saved: &EventConfiguration,
) -> Result<(), GenicamError> {
    if let Some(selector) = &saved.event_selector {
        set_if_changed(camera, EVENT_SELECTOR, selector)?;
    }
    if let Some((name, previous, _)) = saved.channel.as_ref().and_then(|c| c.selector.as_ref()) {
        set_if_changed(camera, name, previous)?;
    }
    Ok(())
}

fn set_if_changed<T: RegisterIo>(
    camera: &mut Camera<T>,
    name: &str,
    value: &str,
) -> Result<(), GenicamError> {
    // This setup/restore boundary needs the device's current selected value,
    // not a feature cache populated before another selector or a raw fallback.
    camera.invalidate_all();
    if camera.get(name)? != value {
        camera.set(name, value)?;
    }
    Ok(())
}

fn restore_step(
    permission: &Permission,
    report: &mut EventRestoreReport,
    setting: &str,
    restore: impl FnOnce() -> Result<(), GenicamError>,
) -> bool {
    if let Some(reason) = permission.skipped() {
        report.skipped = Some(reason);
        return false;
    }
    match restore() {
        Ok(()) => true,
        Err(error) => {
            report.failures.push(EventRestoreFailure {
                setting: setting.into(),
                error,
            });
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::HashMap;

    // Two selected notifications share one register, as real SFNC selectors
    // do. One was already enabled before our registration.
    const XML: &str = r#"<RegisterDescription SchemaMajorVersion="1" SchemaMinorVersion="0" SchemaSubMinorVersion="0">
      <Enumeration Name="EventSelector"><EnumEntry Name="First"><Value>1</Value></EnumEntry><EnumEntry Name="Second"><Value>2</Value></EnumEntry><pValue>SelectorReg</pValue></Enumeration>
      <IntReg Name="SelectorReg"><Address>0x100</Address><Length>4</Length><AccessMode>RW</AccessMode><Endianess>BigEndian</Endianess><Cachable>NoCache</Cachable></IntReg>
      <Enumeration Name="EventNotification"><EnumEntry Name="Off"><Value>0</Value></EnumEntry><EnumEntry Name="On"><Value>1</Value></EnumEntry><pValue>NotificationReg</pValue></Enumeration>
      <IntReg Name="NotificationReg"><Address>0x104</Address><Length>4</Length><AccessMode>RW</AccessMode><Endianess>BigEndian</Endianess><Cachable>NoCache</Cachable></IntReg>
    </RegisterDescription>"#;

    #[derive(Debug)]
    struct MemoryIo {
        values: RefCell<HashMap<u64, u32>>,
        notifications: RefCell<HashMap<u32, u32>>,
        channels: RefCell<HashMap<(u32, u64), u32>>,
        writes: RefCell<Vec<(u64, u32)>>,
        fail: RefCell<Option<(u64, u32, u32)>>,
    }

    impl Default for MemoryIo {
        fn default() -> Self {
            Self {
                values: RefCell::new(HashMap::from([
                    (0x100, 2),
                    (MESSAGE_DESTINATION_ADDRESS, 0xc000_0201),
                    (MESSAGE_DESTINATION_PORT, 4100),
                ])),
                notifications: RefCell::new(HashMap::from([(1, 0), (2, 1)])),
                channels: RefCell::new(HashMap::from([
                    ((0, 0x200), 0xc000_0202),
                    ((0, 0x204), 4200),
                    ((0, 0x208), 0),
                    ((1, 0x200), 0xc000_0203),
                    ((1, 0x204), 4300),
                    ((1, 0x208), 1),
                ])),
                writes: RefCell::new(Vec::new()),
                fail: RefCell::new(None),
            }
        }
    }

    impl RegisterIo for MemoryIo {
        fn read(&self, addr: u64, len: usize) -> Result<Vec<u8>, GenApiError> {
            assert_eq!(len, 4);
            let selector = self.values.borrow()[&0x100];
            let value = if addr == 0x104 {
                self.notifications.borrow()[&selector]
            } else if matches!(addr, 0x200 | 0x204 | 0x208) {
                let channel = *self.values.borrow().get(&0x1fc).unwrap_or(&0);
                self.channels.borrow()[&(channel, addr)]
            } else {
                *self.values.borrow().get(&addr).unwrap_or(&0)
            };
            Ok(value.to_be_bytes().to_vec())
        }

        fn write(&self, addr: u64, data: &[u8]) -> Result<(), GenApiError> {
            let value = u32::from_be_bytes(data.try_into().unwrap());
            let mut fail = self.fail.borrow_mut();
            if let Some((fail_addr, fail_value, remaining)) = fail.as_mut()
                && (*fail_addr, *fail_value) == (addr, value)
            {
                if *remaining == 0 {
                    *fail = None;
                    return Err(GenApiError::io(std::io::Error::other(
                        "injected write failure",
                    )));
                }
                *remaining -= 1;
            }
            drop(fail);
            self.writes.borrow_mut().push((addr, value));
            if addr == 0x104 {
                let selector = self.values.borrow()[&0x100];
                self.notifications.borrow_mut().insert(selector, value);
            } else if matches!(addr, 0x200 | 0x204 | 0x208) {
                let channel = *self.values.borrow().get(&0x1fc).unwrap_or(&0);
                self.channels.borrow_mut().insert((channel, addr), value);
            } else {
                self.values.borrow_mut().insert(addr, value);
            }
            Ok(())
        }
    }

    fn camera() -> Camera<MemoryIo> {
        Camera::new(
            MemoryIo::default(),
            crate::NodeMap::try_from_xml(viva_genapi_xml::parse(XML).unwrap()).unwrap(),
        )
    }

    fn token(camera: &Camera<MemoryIo>) -> EventConfiguration {
        EventConfiguration {
            identity: Arc::downgrade(&camera.event_identity),
            permission: Permission::Memory,
            event_selector: None,
            notifications: Vec::new(),
            channel: None,
        }
    }

    fn prepare(camera: &mut Camera<MemoryIo>) -> EventConfiguration {
        let mut token = token(camera);
        camera
            .snapshot_notifications(&["First", "Second"], &mut token)
            .unwrap();
        camera.snapshot_channel(&mut token).unwrap();
        token
    }

    fn change(camera: &mut Camera<MemoryIo>) {
        camera.enable_events(&["First", "Second"]).unwrap();
        crate::events::configure_message_channel_raw(camera.transport(), Ipv4Addr::LOCALHOST, 5000)
            .unwrap();
    }

    #[test]
    fn restoring_events_recovers_each_selected_notification_and_actual_channel() {
        let mut camera = camera();
        let saved = prepare(&mut camera);
        assert_eq!(camera.get(EVENT_SELECTOR).unwrap(), "Second");
        change(&mut camera);
        assert_eq!(camera.transport.notifications.borrow()[&1], 1);
        let report = camera.restore_events(saved);
        assert!(report.is_complete(), "{report:?}");
        assert_eq!(camera.get(EVENT_SELECTOR).unwrap(), "Second");
        assert_eq!(
            *camera.transport.notifications.borrow(),
            HashMap::from([(1, 0), (2, 1)])
        );
        assert_eq!(
            camera.transport.values.borrow()[&MESSAGE_DESTINATION_ADDRESS],
            0xc000_0201
        );
        assert_eq!(
            camera.transport.values.borrow()[&MESSAGE_DESTINATION_PORT],
            4100
        );
    }

    #[test]
    fn invalid_event_selection_writes_nothing() {
        let mut camera = camera();
        let mut saved = token(&camera);
        let error = camera
            .snapshot_notifications(&["First", "Unknown"], &mut saved)
            .unwrap_err();
        assert!(matches!(
            error,
            GenicamError::GenApi(GenApiError::EnumNoSuchEntry { .. })
        ));
        let report = camera.restore_events(saved);
        assert!(report.is_complete());
        assert!(camera.transport.writes.borrow().is_empty());
    }

    #[test]
    fn partial_notification_setup_rolls_back_without_hiding_original_failure() {
        let mut camera = camera();
        let saved = prepare(&mut camera);
        // Make the second originally-off for this case, then fail enabling it
        // after the first one was changed.
        camera.transport.notifications.borrow_mut().insert(2, 0);
        camera.transport.fail.replace(Some((0x104, 1, 1)));
        let source = camera.enable_events(&["First", "Second"]).unwrap_err();
        let error = camera.setup_error(source, saved);
        assert!(matches!(
            error.source,
            GenicamError::GenApi(GenApiError::Io(_))
        ));
        assert!(error.restoration.is_complete(), "{:?}", error.restoration);
        assert_eq!(camera.transport.notifications.borrow()[&1], 0);
        assert_eq!(camera.get(EVENT_SELECTOR).unwrap(), "Second");
    }

    #[test]
    fn restoration_reports_a_failure_and_still_restores_independent_channel_settings() {
        let mut camera = camera();
        let saved = prepare(&mut camera);
        change(&mut camera);
        camera.transport.fail.replace(Some((0x104, 0, 0)));
        let report = camera.restore_events(saved);
        assert!(!report.is_complete());
        assert_eq!(report.failures.len(), 1);
        assert_eq!(report.failures[0].setting, "EventNotification[First]");
        assert_eq!(
            camera.transport.values.borrow()[&MESSAGE_DESTINATION_PORT],
            4100
        );
    }

    #[test]
    fn a_registration_cannot_be_restored_onto_a_replacement_camera() {
        let mut old = camera();
        let saved = prepare(&mut old);
        let mut replacement = camera();
        let report = replacement.restore_events(saved);
        assert_eq!(report.skipped, Some(EventRestoreSkipped::DifferentCamera));
        assert!(replacement.transport.writes.borrow().is_empty());
    }

    #[test]
    fn lost_released_and_monitor_sessions_perform_no_restoration_io() {
        for state in [
            ControlState::Lost,
            ControlState::Released,
            ControlState::Monitor,
        ] {
            let mut camera = camera();
            let mut saved = prepare(&mut camera);
            change(&mut camera);
            let (control, observed) = tokio::sync::watch::channel(ControlState::Held);
            saved.permission = Permission::Gige(observed);
            control.send_replace(state);
            camera.transport.writes.borrow_mut().clear();
            let report = camera.restore_events(saved);
            assert_eq!(
                report.skipped,
                Some(EventRestoreSkipped::NotInControl(state))
            );
            assert!(camera.transport.writes.borrow().is_empty());
        }
    }

    #[test]
    fn sfnc_channel_settings_and_both_selectors_are_restored_after_raw_fallback() {
        let extra = r#"
          <Enumeration Name="GevMessageChannelSelector"><EnumEntry Name="Zero"><Value>0</Value></EnumEntry><EnumEntry Name="One"><Value>1</Value></EnumEntry><pValue>ChannelSelectorReg</pValue></Enumeration>
          <IntReg Name="ChannelSelectorReg"><Address>0x1FC</Address><Length>4</Length><AccessMode>RW</AccessMode><Endianess>BigEndian</Endianess><Cachable>NoCache</Cachable></IntReg>
          <Integer Name="GevMessageChannelIPAddress"><pValue>ChannelIP</pValue></Integer>
          <IntReg Name="ChannelIP"><Address>0x200</Address><Length>4</Length><AccessMode>RW</AccessMode><Endianess>BigEndian</Endianess><Cachable>NoCache</Cachable></IntReg>
          <Integer Name="GevMessageChannelPort"><pValue>ChannelPort</pValue></Integer>
          <IntReg Name="ChannelPort"><Address>0x204</Address><Length>4</Length><AccessMode>RW</AccessMode><Endianess>BigEndian</Endianess><Cachable>NoCache</Cachable></IntReg>
          <Boolean Name="GevMessageChannelEnable"><pValue>ChannelEnabled</pValue></Boolean>
          <IntReg Name="ChannelEnabled"><Address>0x208</Address><Length>4</Length><AccessMode>RW</AccessMode><Endianess>BigEndian</Endianess><Cachable>NoCache</Cachable></IntReg>
        "#;
        let xml = XML.replace(
            "</RegisterDescription>",
            &format!("{extra}</RegisterDescription>"),
        );
        let io = MemoryIo::default();
        io.values.borrow_mut().insert(0x1fc, 1);
        let mut camera = Camera::new(
            io,
            crate::NodeMap::try_from_xml(viva_genapi_xml::parse(&xml).unwrap()).unwrap(),
        );
        let original_channels = camera.transport.channels.borrow().clone();
        let saved = prepare(&mut camera);
        assert_eq!(camera.get("GevMessageChannelSelector").unwrap(), "One");
        // Partial SFNC setup followed by the existing raw-bootstrap fallback.
        camera.set("GevMessageChannelSelector", "Zero").unwrap();
        camera
            .set("GevMessageChannelIPAddress", "2130706433")
            .unwrap();
        camera.set("GevMessageChannelPort", "5000").unwrap();
        camera.set("GevMessageChannelEnable", "true").unwrap();
        change(&mut camera);
        let report = camera.restore_events(saved);
        assert!(report.is_complete(), "{report:?}");
        assert_eq!(*camera.transport.channels.borrow(), original_channels);
        assert_eq!(camera.get("GevMessageChannelSelector").unwrap(), "One");
        assert_eq!(camera.get(EVENT_SELECTOR).unwrap(), "Second");
        assert_eq!(
            camera.transport.values.borrow()[&MESSAGE_DESTINATION_PORT],
            4100
        );
    }
}
