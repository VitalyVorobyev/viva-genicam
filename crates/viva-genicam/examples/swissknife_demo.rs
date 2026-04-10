use std::cell::RefCell;
use std::collections::HashMap;
use std::env;
use std::error::Error;
use std::net::{IpAddr, SocketAddr};
use std::time::Duration;

use viva_genapi_xml::{self, NodeDecl, SkOutput};
use viva_genicam::genapi::{GenApiError, NodeMap, RegisterIo};
use viva_genicam::gige::GigeDevice;
use viva_genicam::{Camera, GigeRegisterIo};

fn main() -> Result<(), Box<dyn Error>> {
    if env::args().any(|arg| arg == "--mock") {
        run_mock()?;
    } else {
        run_real()?;
    }
    Ok(())
}

fn run_mock() -> Result<(), Box<dyn Error>> {
    const XML: &str = r#"
        <RegisterDescription SchemaMajorVersion="1" SchemaMinorVersion="0" SchemaSubMinorVersion="0">
            <Integer Name="GainRaw">
                <Address>0x3000</Address>
                <Length>4</Length>
                <AccessMode>RW</AccessMode>
                <Min>0</Min>
                <Max>1000</Max>
            </Integer>
            <Float Name="Offset">
                <Address>0x3008</Address>
                <Length>4</Length>
                <AccessMode>RW</AccessMode>
                <Min>-100.0</Min>
                <Max>100.0</Max>
            </Float>
            <SwissKnife Name="ComputedGain">
                <Expression>(GainRaw * 0.5) + Offset</Expression>
                <pVariable Name="GainRaw">GainRaw</pVariable>
                <pVariable Name="Offset">Offset</pVariable>
                <Output>Float</Output>
            </SwissKnife>
        </RegisterDescription>
    "#;

    let model = viva_genapi_xml::parse(XML)?;
    let mut nodemap = NodeMap::try_from_xml(model)?;
    let io = MockIo::new(&[(0x3000, 4), (0x3008, 4)]);

    nodemap.set_integer("GainRaw", 100, &io)?;
    nodemap.set_float("Offset", 3.0, &io)?;

    let value = nodemap.get_float("ComputedGain", &io)?;
    println!("ComputedGain (initial) -> {:.1}", value);

    nodemap.set_integer("GainRaw", 120, &io)?;
    let updated = nodemap.get_float("ComputedGain", &io)?;
    println!("ComputedGain (after GainRaw=120) -> {:.1}", updated);

    Ok(())
}

fn run_real() -> Result<(), Box<dyn Error>> {
    let rt = tokio::runtime::Runtime::new()?;
    let devices = rt.block_on(viva_genicam::gige::discover(Duration::from_secs(1)))?;
    let Some(info) = devices.first() else {
        println!("No GigE Vision devices discovered.");
        return Ok(());
    };

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
    let swissknife_nodes: Vec<(String, SkOutput)> = model
        .nodes
        .iter()
        .filter_map(|decl| match decl {
            NodeDecl::SwissKnife(sk) => Some((sk.name.clone(), sk.output)),
            _ => None,
        })
        .collect();

    if swissknife_nodes.is_empty() {
        println!("No SwissKnife nodes reported by the device XML.");
        return Ok(());
    }

    let nodemap = NodeMap::try_from_xml(model)?;
    let handle = rt.handle().clone();
    let device = match std::sync::Arc::try_unwrap(device) {
        Ok(mutex) => mutex.into_inner(),
        Err(_) => panic!("device still has outstanding clones"),
    };
    let transport = GigeRegisterIo::new(handle, device);
    let camera = Camera::new(transport, nodemap);

    println!("SwissKnife nodes:");
    for (name, output) in swissknife_nodes {
        match output {
            SkOutput::Float => match camera.nodemap().get_float(&name, camera.transport()) {
                Ok(value) => println!("  {name} -> {value}"),
                Err(err) => println!("  {name} -> error: {err}"),
            },
            SkOutput::Integer => match camera.nodemap().get_integer(&name, camera.transport()) {
                Ok(value) => println!("  {name} -> {value}"),
                Err(err) => println!("  {name} -> error: {err}"),
            },
        }
    }

    Ok(())
}

#[derive(Default)]
struct MockIo {
    regs: RefCell<HashMap<u64, Vec<u8>>>,
}

impl MockIo {
    fn new(layout: &[(u64, usize)]) -> Self {
        let mut regs = HashMap::new();
        for (addr, len) in layout {
            regs.insert(*addr, vec![0u8; *len]);
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
