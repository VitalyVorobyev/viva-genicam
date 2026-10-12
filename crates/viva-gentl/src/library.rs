//! The library's state: `GCInitLib` to `GCCloseLib`, the runtime every
//! camera runs on, and the registry of open modules that handles name.
//!
//! A handle is a key into the registry, cast to `void *`; nothing ever
//! dereferences one (ADR-0031). Keys are never reused, so a stale handle —
//! from a closed module, or from a library session that ended — finds
//! nothing and is `GC_ERR_INVALID_HANDLE`.

use std::collections::HashMap;
use std::ffi::c_void;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::abi::*;
use crate::device::Device;
use crate::error::{Failure, Result, invalid_handle, poisoned};
use crate::interface::Interface;
use crate::stream::Stream;
use crate::system::System;

/// The open library, while there is one.
static LIBRARY: Mutex<Option<Arc<Library>>> = Mutex::new(None);

/// The next handle. Starts far from small integers and is never reused,
/// across library sessions too.
static NEXT_HANDLE: AtomicUsize = AtomicUsize::new(0x5649_5641_0000);

/// How long `GCCloseLib` waits for the runtime's threads to finish.
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(2);

/// An open module, as the registry holds it.
#[derive(Clone)]
pub(crate) enum Object {
    System(Arc<System>),
    Interface(Arc<Interface>),
    Device(Arc<Device>),
    /// The remote device's port (`DevGetPort`), a child of its device.
    RemotePort(Arc<Device>),
    Stream(Arc<Stream>),
    /// An announced buffer, a child of its stream.
    Buffer(Arc<Stream>),
    /// A registered `EVENT_NEW_BUFFER`, a child of its stream.
    NewBufferEvent(Arc<Stream>),
}

struct Entry {
    parent: Option<usize>,
    object: Object,
}

/// The state between `GCInitLib` and `GCCloseLib`.
pub(crate) struct Library {
    /// Where every camera's socket, keepalive and stream setup run.
    pub(crate) runtime: tokio::runtime::Handle,
    owned_runtime: Mutex<Option<tokio::runtime::Runtime>>,
    registry: Mutex<HashMap<usize, Entry>>,
}

/// `GCInitLib`: a second call without `GCCloseLib` is
/// `GC_ERR_RESOURCE_IN_USE` (§3.1).
pub(crate) fn init() -> Result<()> {
    init_logging();
    let mut library = LIBRARY.lock().map_err(poisoned)?;
    if library.is_some() {
        return Err(Failure::new(
            GC_ERR_RESOURCE_IN_USE,
            "GCInitLib was already called; call GCCloseLib first",
        ));
    }
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .thread_name("viva-gentl")
        .enable_all()
        .build()
        .map_err(|err| {
            Failure::new(GC_ERR_RESOURCE_EXHAUSTED, format!("start a runtime: {err}"))
        })?;
    *library = Some(Arc::new(Library {
        runtime: runtime.handle().clone(),
        owned_runtime: Mutex::new(Some(runtime)),
        registry: Mutex::new(HashMap::new()),
    }));
    Ok(())
}

/// With `RUST_LOG` set, log to standard error as the rest of viva does: a
/// consumer is rarely a Rust program, so no one else installs a subscriber
/// for the facade's logs. Once per process; a subscriber already installed
/// (a Rust consumer's) is kept.
fn init_logging() {
    if std::env::var_os("RUST_LOG").is_some() {
        let _ = tracing_subscriber::fmt()
            .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
            .with_writer(std::io::stderr)
            .try_init();
    }
}

/// `GCCloseLib`: close every module still open, release every camera, join
/// every thread, and shut the runtime down.
pub(crate) fn close() -> Result<()> {
    let library = LIBRARY
        .lock()
        .map_err(poisoned)?
        .take()
        .ok_or_else(not_initialized)?;
    let roots: Vec<usize> = {
        let registry = library.registry.lock().map_err(poisoned)?;
        registry
            .iter()
            .filter(|(_, entry)| entry.parent.is_none())
            .map(|(&handle, _)| handle)
            .collect()
    };
    for root in roots {
        library.close(root)?;
    }
    let runtime = library.owned_runtime.lock().map_err(poisoned)?.take();
    if let Some(runtime) = runtime {
        runtime.shutdown_timeout(SHUTDOWN_TIMEOUT);
    }
    Ok(())
}

fn not_initialized() -> Failure {
    Failure::new(GC_ERR_NOT_INITIALIZED, "GCInitLib has not been called")
}

/// The open library, or `GC_ERR_NOT_INITIALIZED`.
pub(crate) fn get() -> Result<Arc<Library>> {
    LIBRARY
        .lock()
        .map_err(poisoned)?
        .clone()
        .ok_or_else(not_initialized)
}

/// A handle as the consumer sees it.
pub(crate) fn handle(key: usize) -> Handle {
    key as *mut c_void
}

impl Library {
    /// A fresh key for `object`, a child of `parent`.
    pub(crate) fn insert(&self, parent: Option<usize>, object: Object) -> Result<usize> {
        let key = NEXT_HANDLE.fetch_add(0x10, Ordering::Relaxed);
        self.insert_at(key, parent, object)?;
        Ok(key)
    }

    /// Register `object` under a key reserved with [`Library::reserve`].
    pub(crate) fn insert_at(
        &self,
        key: usize,
        parent: Option<usize>,
        object: Object,
    ) -> Result<()> {
        self.registry
            .lock()
            .map_err(poisoned)?
            .insert(key, Entry { parent, object });
        Ok(())
    }

    /// A key for an object that needs to know its own handle before it is
    /// registered.
    pub(crate) fn reserve(&self) -> usize {
        NEXT_HANDLE.fetch_add(0x10, Ordering::Relaxed)
    }

    /// The object behind `handle`, and its parent's key.
    pub(crate) fn lookup(&self, handle: Handle) -> Result<(Object, Option<usize>)> {
        let registry = self.registry.lock().map_err(poisoned)?;
        registry
            .get(&(handle as usize))
            .map(|entry| (entry.object.clone(), entry.parent))
            .ok_or_else(|| invalid_handle("the handle names no open module"))
    }

    /// Whether any open object satisfies `test`.
    pub(crate) fn any(&self, test: impl Fn(&Object) -> bool) -> Result<bool> {
        let registry = self.registry.lock().map_err(poisoned)?;
        Ok(registry.values().any(|entry| test(&entry.object)))
    }

    pub(crate) fn system(&self, handle: Handle) -> Result<Arc<System>> {
        match self.lookup(handle)?.0 {
            Object::System(system) => Ok(system),
            _ => Err(invalid_handle("not a System module handle")),
        }
    }

    pub(crate) fn interface(&self, handle: Handle) -> Result<Arc<Interface>> {
        match self.lookup(handle)?.0 {
            Object::Interface(interface) => Ok(interface),
            _ => Err(invalid_handle("not an Interface module handle")),
        }
    }

    pub(crate) fn device(&self, handle: Handle) -> Result<Arc<Device>> {
        match self.lookup(handle)?.0 {
            Object::Device(device) => Ok(device),
            _ => Err(invalid_handle("not a Device module handle")),
        }
    }

    pub(crate) fn stream(&self, handle: Handle) -> Result<Arc<Stream>> {
        match self.lookup(handle)?.0 {
            Object::Stream(stream) => Ok(stream),
            _ => Err(invalid_handle("not a Data Stream module handle")),
        }
    }

    /// The stream `stream` names, and the key of `buffer`, which must be
    /// one of its buffers.
    pub(crate) fn buffer(&self, stream: Handle, buffer: Handle) -> Result<(Arc<Stream>, usize)> {
        let owner = self.stream(stream)?;
        match self.lookup(buffer)? {
            (Object::Buffer(of), Some(parent))
                if Arc::ptr_eq(&of, &owner) && parent == owner.key =>
            {
                Ok((owner, buffer as usize))
            }
            _ => Err(invalid_handle("not a buffer announced to this data stream")),
        }
    }

    /// The parent key of `handle`.
    pub(crate) fn parent(&self, handle: Handle) -> Result<Option<usize>> {
        Ok(self.lookup(handle)?.1)
    }

    /// Close `key` and everything under it: remove the subtree from the
    /// registry first, then shut its objects down — children before
    /// parents — outside the registry's lock.
    pub(crate) fn close(&self, key: usize) -> Result<()> {
        let removed = {
            let mut registry = self.registry.lock().map_err(poisoned)?;
            if !registry.contains_key(&key) {
                return Err(invalid_handle("the handle names no open module"));
            }
            let mut order = vec![key];
            let mut next = 0;
            while next < order.len() {
                let parent = order[next];
                order.extend(
                    registry
                        .iter()
                        .filter(|(_, entry)| entry.parent == Some(parent))
                        .map(|(&child, _)| child),
                );
                next += 1;
            }
            order
                .into_iter()
                .filter_map(|k| registry.remove(&k).map(|entry| (k, entry)))
                .collect::<Vec<_>>()
        };
        for (key, entry) in removed.into_iter().rev() {
            self.shut_down(key, entry);
        }
        Ok(())
    }

    /// Remove one leaf (a buffer, an event) without shutting anything down.
    pub(crate) fn forget(&self, key: usize) -> Result<()> {
        self.registry.lock().map_err(poisoned)?.remove(&key);
        Ok(())
    }

    fn shut_down(&self, key: usize, entry: Entry) {
        match entry.object {
            Object::Stream(stream) => stream.shut_down(),
            Object::Device(device) => device.shut_down(),
            Object::NewBufferEvent(stream) => stream.unregister_event(key),
            Object::System(_)
            | Object::Interface(_)
            | Object::RemotePort(_)
            | Object::Buffer(_) => {}
        }
        if let Some(parent) = entry.parent {
            self.child_closed(parent, key);
        }
    }

    /// Tell `parent` its child `key` closed, so the child's ID opens again.
    fn child_closed(&self, parent: usize, key: usize) {
        let Ok(registry) = self.registry.lock() else {
            return;
        };
        let parent = registry.get(&parent).map(|entry| entry.object.clone());
        drop(registry);
        match parent {
            Some(Object::System(system)) => system.child_closed(key),
            Some(Object::Interface(interface)) => interface.child_closed(key),
            Some(Object::Device(device)) => device.child_closed(key),
            _ => {}
        }
    }
}
