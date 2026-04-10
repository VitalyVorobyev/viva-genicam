use std::cell::RefCell;
use std::collections::HashMap;
use std::env;
use std::error::Error;
use std::net::{IpAddr, SocketAddr};
use std::time::Duration;

use std::convert::TryInto;

use viva_genicam::genapi::{GenApiError, NodeMap, RegisterIo};
use viva_genicam::gige::GigeDevice;
use viva_genicam::{Camera, GigeRegisterIo};

const MODE_ADDR: u64 = 0x4000;
const PROVIDER_ADDR: u64 = 0x4100;

const MOCK_XML: &str = r#"
    <RegisterDescription SchemaMajorVersion="1" SchemaMinorVersion="0" SchemaSubMinorVersion="0">
        <Enumeration Name="Mode">
            <Address>0x4000</Address>
            <Length>4</Length>
            <AccessMode>RW</AccessMode>
            <EnumEntry Name="Fixed10">
                <Value>10</Value>
            </EnumEntry>
            <EnumEntry Name="DynFromReg">
                <pValue>RegModeVal</pValue>
            </EnumEntry>
        </Enumeration>
        <Integer Name="RegModeVal">
            <Address>0x4100</Address>
            <Length>4</Length>
            <AccessMode>RW</AccessMode>
            <Min>0</Min>
            <Max>65535</Max>
        </Integer>
    </RegisterDescription>
"#;

fn main() -> Result<(), Box<dyn Error>> {
    if env::args().any(|arg| arg == "--real") {
        run_real()?;
    } else {
        run_mock()?;
    }
    Ok(())
}

fn run_mock() -> Result<(), Box<dyn Error>> {
    println!("-- mock enum pValue demo --");

    let camera = build_mock_camera(10, 42);
    println!("Entries: {:?}", camera.enum_entries("Mode")?);
    println!("Case 1: Mode register=10 -> {}", camera.get("Mode")?);

    let camera = build_mock_camera(42, 42);
    println!("Case 2: Mode register=42 -> {}", camera.get("Mode")?);

    let mut camera = build_mock_camera(0, 42);
    camera.set("Mode", "DynFromReg")?;
    println!(
        "Case 3: set DynFromReg -> raw={} ({}).",
        camera.transport().read_u32(MODE_ADDR),
        camera.get("Mode")?
    );

    let mut camera = build_mock_camera(0, 42);
    camera.set("RegModeVal", "17")?;
    camera.set("Mode", "DynFromReg")?;
    println!(
        "Case 4: provider=17 -> raw={} ({}).",
        camera.transport().read_u32(MODE_ADDR),
        camera.get("Mode")?
    );

    Ok(())
}

fn run_real() -> Result<(), Box<dyn Error>> {
    let rt = tokio::runtime::Runtime::new()?;
    let devices = rt.block_on(viva_genicam::gige::discover(Duration::from_secs(1)))?;
    let Some(info) = devices.first() else {
        println!("No GigE Vision devices discovered.");
        return Ok(());
    };

    println!(
        "Connecting to {}",
        info.model.clone().unwrap_or_else(|| "camera".into())
    );
    let addr = SocketAddr::new(IpAddr::V4(info.ip), viva_genicam::gige::GVCP_PORT);
    let device = rt.block_on(GigeDevice::open(addr))?;
    let device = std::sync::Arc::new(tokio::sync::Mutex::new(device));
    let xml = rt.block_on(viva_genapi_xml::fetch_and_load_xml({
        let device = device.clone();
        move |address, length| {
            let device = device.clone();
            async move {
                let mut dev = device.lock().await;
                dev.read_mem(address, length)
                    .await
                    .map_err(|err| viva_genapi_xml::XmlError::Transport(err.to_string()))
            }
        }
    }))?;
    let model = viva_genapi_xml::parse(&xml)?;
    let nodemap = NodeMap::from(model);
    let handle = rt.handle().clone();
    let device = match std::sync::Arc::try_unwrap(device) {
        Ok(mutex) => mutex.into_inner(),
        Err(_) => panic!("device still has outstanding clones"),
    };
    let transport = GigeRegisterIo::new(handle, device);
    let mut camera = Camera::new(transport, nodemap);

    let candidates = [
        "TriggerSelector",
        "ExposureAuto",
        "GainSelector",
        "PixelFormat",
    ];
    let mut selected = None;
    for name in candidates {
        match camera.enum_entries(name) {
            Ok(entries) if !entries.is_empty() => {
                println!("{name} entries: {entries:?}");
                selected = Some((name.to_string(), entries));
                break;
            }
            Ok(_) => {}
            Err(err) => println!("Skipping {name}: {err}"),
        }
    }

    if let Some((name, entries)) = selected {
        match camera.get(&name) {
            Ok(current) => println!("Current {name} -> {current}"),
            Err(err) => println!("Unable to read {name}: {err}"),
        }
        if let Some(target) = entries.first() {
            println!("Attempting to set {name} to {target}");
            if let Err(err) = camera.set(&name, target) {
                println!("  Failed to set {name}: {err}");
            } else {
                match camera.get(&name) {
                    Ok(updated) => println!("  Updated {name} -> {updated}"),
                    Err(err) => println!("  Unable to confirm {name}: {err}"),
                }
            }
        }
    } else {
        println!("No common enumeration features found.");
    }

    Ok(())
}

fn build_mock_camera(mode_value: u32, provider_value: u32) -> Camera<MockIo> {
    let model = viva_genapi_xml::parse(MOCK_XML).expect("parse mock xml");
    let nodemap = NodeMap::from(model);
    let mut transport = MockIo::new();
    transport.set_u32(MODE_ADDR, mode_value);
    transport.set_u32(PROVIDER_ADDR, provider_value);
    Camera::new(transport, nodemap)
}

struct MockIo {
    regs: RefCell<HashMap<u64, Vec<u8>>>,
}

impl MockIo {
    fn new() -> Self {
        Self {
            regs: RefCell::new(HashMap::new()),
        }
    }

    fn set_u32(&mut self, addr: u64, value: u32) {
        self.regs
            .borrow_mut()
            .insert(addr, value.to_be_bytes().to_vec());
    }

    fn read_u32(&self, addr: u64) -> u32 {
        let regs = self.regs.borrow();
        let data = regs.get(&addr).cloned().unwrap_or_else(|| vec![0; 4]);
        u32::from_be_bytes(data.try_into().expect("u32 width"))
    }
}

impl RegisterIo for MockIo {
    fn read(&self, addr: u64, len: usize) -> Result<Vec<u8>, GenApiError> {
        let regs = self.regs.borrow();
        let data = regs.get(&addr).cloned().unwrap_or_else(|| vec![0; len]);
        if data.len() != len {
            return Err(GenApiError::Io(format!(
                "length mismatch at 0x{addr:08X}: expected {len}, have {}",
                data.len()
            )));
        }
        Ok(data)
    }

    fn write(&self, addr: u64, data: &[u8]) -> Result<(), GenApiError> {
        self.regs.borrow_mut().insert(addr, data.to_vec());
        Ok(())
    }
}
