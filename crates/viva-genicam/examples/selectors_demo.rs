use std::cell::RefCell;
use std::collections::HashMap;
use std::env;
use std::error::Error;
use std::net::{IpAddr, SocketAddr};
use std::time::Duration;

use viva_genapi_xml::Addressing;
use viva_genicam::genapi::{GenApiError, Node, NodeMap, RegisterIo};
use viva_genicam::gige::GigeDevice;
use viva_genicam::sfnc;
use viva_genicam::{Camera, GenicamError, GigeRegisterIo};

fn main() -> Result<(), Box<dyn Error>> {
    if env::args().any(|arg| arg == "--real") {
        run_real()?;
    } else {
        run_mock()?;
    }
    Ok(())
}

fn run_mock() -> Result<(), Box<dyn Error>> {
    const XML: &str = r#"
        <RegisterDescription SchemaMajorVersion="1" SchemaMinorVersion="2" SchemaSubMinorVersion="3">
            <Enumeration Name="GainSelector">
                <Address>0x300</Address>
                <Length>2</Length>
                <AccessMode>RW</AccessMode>
                <EnumEntry Name="All" Value="0" />
                <EnumEntry Name="Red" Value="1" />
                <EnumEntry Name="Blue" Value="2" />
            </Enumeration>
            <Integer Name="Gain">
                <Length>2</Length>
                <AccessMode>RW</AccessMode>
                <Min>0</Min>
                <Max>48</Max>
                <pSelected>GainSelector</pSelected>
                <Selected>All</Selected>
                <Address>0x310</Address>
                <Selected>Red</Selected>
                <Address>0x314</Address>
                <Selected>Blue</Selected>
            </Integer>
        </RegisterDescription>
    "#;

    let model = viva_genapi_xml::parse(XML)?;
    let nodemap = NodeMap::from(model);
    let transport = MockIo::with_registers(&[
        (0x300, vec![0, 0]),
        (0x310, vec![0, 12]),
        (0x314, vec![0, 28]),
    ]);
    let mut camera = Camera::new(transport, nodemap);

    println!("-- mock Gain selector demo --");
    print_gain_addressing(camera.nodemap());

    let scenarios = ["All", "Red", "Blue"];
    for selector in scenarios {
        if selector != "All" {
            camera.set(sfnc::GAIN_SELECTOR, selector)?;
        }
        println!("\nSelector -> {}", camera.get(sfnc::GAIN_SELECTOR)?);
        match camera.get(sfnc::GAIN) {
            Ok(value) => println!("Gain ({selector}) -> {value}"),
            Err(GenicamError::GenApi(GenApiError::Unavailable(msg))) => {
                println!("Gain unavailable: {msg}");
            }
            Err(err) => return Err(err.into()),
        }
    }

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

    println!("-- real Gain selector demo --");
    print_gain_addressing(camera.nodemap());

    let selectors = ["All", "Red"];
    for selector in selectors {
        println!("\nAttempting GainSelector={selector}");
        if let Err(err) = camera.set(sfnc::GAIN_SELECTOR, selector) {
            println!("  Unable to set selector: {err}");
            continue;
        }
        match camera.get(sfnc::GAIN) {
            Ok(value) => println!("  Gain -> {value}"),
            Err(GenicamError::GenApi(GenApiError::Unavailable(msg))) => {
                println!("  Gain unavailable: {msg}");
            }
            Err(err) => println!("  Failed to read Gain: {err}"),
        }
    }

    Ok(())
}

fn print_gain_addressing(nodemap: &NodeMap) {
    if let Some(Node::Integer(node)) = nodemap.node(sfnc::GAIN) {
        match &node.addressing {
            Some(Addressing::Fixed { address, len }) => {
                println!("Gain uses fixed address 0x{address:08X} ({} bytes)", len);
            }
            Some(Addressing::BySelector { selector, map }) => {
                println!("Gain addresses by selector {selector}:");
                for (value, (addr, len)) in map {
                    println!("  {value:>8} -> 0x{addr:08X} ({} bytes)", len);
                }
            }
            Some(Addressing::Indirect {
                p_address_node,
                len,
            }) => {
                println!(
                    "Gain resolves address via {p_address_node} ({} bytes per register)",
                    len
                );
            }
            None => {
                println!("Gain has no direct addressing (pValue-backed)");
            }
        }
    }
}

struct MockIo {
    regs: RefCell<HashMap<u64, Vec<u8>>>,
}

impl MockIo {
    fn with_registers(entries: &[(u64, Vec<u8>)]) -> Self {
        let mut regs = HashMap::new();
        for (addr, data) in entries {
            regs.insert(*addr, data.clone());
        }
        MockIo {
            regs: RefCell::new(regs),
        }
    }
}

impl RegisterIo for MockIo {
    fn read(&self, addr: u64, len: usize) -> Result<Vec<u8>, GenApiError> {
        let regs = self.regs.borrow();
        let data = regs
            .get(&addr)
            .ok_or_else(|| GenApiError::Io(format!("read miss at 0x{addr:08X}")))?;
        if data.len() != len {
            return Err(GenApiError::Io(format!(
                "length mismatch at 0x{addr:08X}: expected {len}, have {}",
                data.len()
            )));
        }
        Ok(data.clone())
    }

    fn write(&self, addr: u64, data: &[u8]) -> Result<(), GenApiError> {
        self.regs.borrow_mut().insert(addr, data.to_vec());
        Ok(())
    }
}
