//! A selector and the feature it selects (`<pSelected>`, `<pIndex>`), on an in-memory register map or (`--real`) a camera.

use std::cell::RefCell;
use std::collections::HashMap;
use std::env;
use std::error::Error;
use std::net::{IpAddr, SocketAddr};
use std::time::Duration;

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
    // The standard selector shape (GenICam Standard v2.1.1 §2.8.4): the
    // selector names what it selects in `<pSelected>`, and the selected
    // feature's address steps through `<pIndex>` on the selector's register.
    const XML: &str = r#"
        <RegisterDescription SchemaMajorVersion="1" SchemaMinorVersion="2" SchemaSubMinorVersion="3">
            <Enumeration Name="GainSelector">
                <EnumEntry Name="All" Value="0" />
                <EnumEntry Name="Red" Value="1" />
                <EnumEntry Name="Blue" Value="2" />
                <pValue>GainSelectorReg</pValue>
                <pSelected>Gain</pSelected>
            </Enumeration>
            <IntReg Name="GainSelectorReg">
                <Address>0x300</Address>
                <Length>2</Length>
                <AccessMode>RW</AccessMode>
                <Sign>Unsigned</Sign>
                <Endianess>BigEndian</Endianess>
            </IntReg>
            <Integer Name="Gain">
                <Address>0x310</Address>
                <pIndex Offset="4">GainSelectorReg</pIndex>
                <Length>2</Length>
                <AccessMode>RW</AccessMode>
                <Min>0</Min>
                <Max>48</Max>
            </Integer>
        </RegisterDescription>
    "#;

    let model = viva_genapi_xml::parse(XML)?;
    let nodemap = NodeMap::try_from_xml(model)?;
    let transport = MockIo::with_registers(&[
        (0x300, vec![0, 0]),
        (0x310, vec![0, 12]),
        (0x314, vec![0, 28]),
        (0x318, vec![0, 31]),
    ]);
    let mut camera = Camera::new(transport, nodemap);

    println!("-- mock Gain selector demo --");

    let scenarios = ["All", "Red", "Blue"];
    for selector in scenarios {
        if selector != "All" {
            camera.set(sfnc::GAIN_SELECTOR, selector)?;
        }
        println!("\nSelector -> {}", camera.get(sfnc::GAIN_SELECTOR)?);
        print_gain_address(&camera);
        println!("Gain ({selector}) -> {}", camera.get(sfnc::GAIN)?);
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
    let mut device = rt.block_on(GigeDevice::open(addr))?;
    // The adapter below keeps control alive and releases it when dropped.
    rt.block_on(device.claim_control())?;
    let device = std::sync::Arc::new(tokio::sync::Mutex::new(device));
    let xml = rt.block_on(viva_genapi_xml::fetch_and_load_xml({
        let device = device.clone();
        move |address, length| {
            let device = device.clone();
            async move {
                let mut dev = device.lock().await;
                dev.read_mem(address, length)
                    .await
                    .map_err(viva_genapi_xml::XmlError::transport)
            }
        }
    }))?;
    let model = viva_genapi_xml::parse(&xml)?;
    let nodemap = NodeMap::try_from_xml(model)?;
    let handle = rt.handle().clone();
    let device = match std::sync::Arc::try_unwrap(device) {
        Ok(mutex) => mutex.into_inner(),
        Err(_) => panic!("device still has outstanding clones"),
    };
    let transport = GigeRegisterIo::new(handle, device);
    let mut camera = Camera::new(transport, nodemap);

    println!("-- real Gain selector demo --");
    print_gain_address(&camera);

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

/// Where `Gain` is for the selector's current entry: its address moves with
/// the selector, through `<pIndex>` on the selector's register. Resolved by
/// name; how the XML spells the address is the node map's business.
fn print_gain_address<T: RegisterIo>(camera: &Camera<T>) {
    if let Some(Node::Enum(selector)) = camera.nodemap().node(sfnc::GAIN_SELECTOR) {
        println!("GainSelector selects {:?}", selector.selected);
    }
    match camera
        .nodemap()
        .register_address(sfnc::GAIN, camera.transport())
    {
        Ok((address, len)) => println!("Gain is {len} bytes at 0x{address:08X}"),
        Err(err) => println!("Gain has no register of its own: {err}"),
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
            .ok_or_else(|| GenApiError::io(format!("read miss at 0x{addr:08X}")))?;
        if data.len() != len {
            return Err(GenApiError::io(format!(
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
