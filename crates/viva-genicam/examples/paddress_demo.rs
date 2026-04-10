use std::cell::RefCell;
use std::collections::HashMap;
use std::env;
use std::error::Error;
use std::net::{IpAddr, SocketAddr};
use std::time::Duration;

use viva_genapi_xml::{self, Addressing, NodeDecl};
use viva_genicam::genapi::{GenApiError, Node, NodeMap, RegisterIo};
use viva_genicam::gige::GigeDevice;
use viva_genicam::{Camera, GigeRegisterIo};

type Result<T> = std::result::Result<T, Box<dyn Error>>;

fn main() -> Result<()> {
    let args: Vec<String> = env::args().collect();
    if args.iter().any(|arg| arg == "--real") {
        run_real()?;
    } else if args.iter().any(|arg| arg == "--mock") {
        run_mock()?;
    } else {
        eprintln!("Usage: paddress_demo --mock | --real");
        eprintln!("  --mock  run against an embedded XML + MockIo transport");
        eprintln!("  --real  attempt discovery against the first GigE Vision camera");
    }
    Ok(())
}

fn run_mock() -> Result<()> {
    const XML: &str = r#"
        <RegisterDescription SchemaMajorVersion="1" SchemaMinorVersion="0" SchemaSubMinorVersion="0">
            <Integer Name="RegAddr">
                <Address>0x2000</Address>
                <Length>4</Length>
                <AccessMode>RW</AccessMode>
                <Min>0</Min>
                <Max>65535</Max>
            </Integer>
            <Integer Name="Gain">
                <pAddress>RegAddr</pAddress>
                <Length>4</Length>
                <AccessMode>RW</AccessMode>
                <Min>0</Min>
                <Max>255</Max>
            </Integer>
        </RegisterDescription>
    "#;

    let model = viva_genapi_xml::parse(XML)?;
    let mut nodemap = NodeMap::from(model);
    let transport = MockIo::with_registers(&[
        (0x2000, vec![0x00, 0x00, 0x30, 0x00]),
        (0x3000, vec![0x00, 0x00, 0x00, 0x7B]),
        (0x3100, vec![0x00, 0x00, 0x00, 0x4D]),
    ]);

    println!("-- mock pAddress demo --");
    print_indirect_nodes(nodemap.node("Gain"));

    let reg_addr = nodemap.get_integer("RegAddr", &transport)? as u64;
    let gain = nodemap.get_integer("Gain", &transport)?;
    println!("RegAddr -> 0x{reg_addr:04X}");
    println!("Gain[0x{reg_addr:04X}] -> {gain}");

    nodemap.set_integer("RegAddr", 0x3100, &transport)?;
    let reg_addr = nodemap.get_integer("RegAddr", &transport)? as u64;
    let gain = nodemap.get_integer("Gain", &transport)?;
    println!("RegAddr -> 0x{reg_addr:04X}");
    println!("Gain[0x{reg_addr:04X}] -> {gain}");

    Ok(())
}

fn run_real() -> Result<()> {
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
    let indirect_integers = collect_indirect_integers(&model);
    if indirect_integers.is_empty() {
        println!("Camera XML does not expose integer nodes with <pAddress>; nothing to do.");
        return Ok(());
    }

    let nodemap = NodeMap::from(model);
    let handle = rt.handle().clone();
    let device = match std::sync::Arc::try_unwrap(device) {
        Ok(mutex) => mutex.into_inner(),
        Err(_) => panic!("device still has outstanding clones"),
    };
    let transport = GigeRegisterIo::new(handle, device);
    let camera = Camera::new(transport, nodemap);

    println!("-- real pAddress demo --");
    for (name, address_node) in indirect_integers {
        println!("Feature {name} resolves via {address_node}");
        match (
            camera.nodemap().node(&address_node),
            camera.nodemap().node(&name),
        ) {
            (Some(Node::Integer(_)), Some(Node::Integer(_))) => {
                match camera
                    .nodemap()
                    .get_integer(&address_node, camera.transport())
                {
                    Ok(addr) => {
                        println!("  {address_node} -> 0x{addr:08X}");
                        match camera.nodemap().get_integer(&name, camera.transport()) {
                            Ok(value) => println!("  {name}[0x{addr:08X}] -> {value}"),
                            Err(err) => println!("  Failed to read {name}: {err}"),
                        }
                    }
                    Err(err) => println!("  Failed to read {address_node}: {err}"),
                }
            }
            _ => println!("  Unsupported node types for {name}; skipping."),
        }
    }

    println!("(No automatic updates performed; addresses displayed as-is.)");

    Ok(())
}

fn collect_indirect_integers(model: &viva_genapi_xml::XmlModel) -> Vec<(String, String)> {
    let mut result = Vec::new();
    for decl in &model.nodes {
        if let NodeDecl::Integer {
            name,
            addressing: Some(Addressing::Indirect { p_address_node, .. }),
            ..
        } = decl
        {
            result.push((name.to_string(), p_address_node.to_string()));
        }
    }
    result
}

fn print_indirect_nodes(node: Option<&Node>) {
    if let Some(Node::Integer(viva_genicam::genapi::IntegerNode {
        addressing:
            Some(Addressing::Indirect {
                p_address_node,
                len,
            }),
        ..
    })) = node
    {
        println!(
            "Gain uses indirect addressing via {p_address_node} ({} bytes)",
            len
        );
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
    fn read(&self, addr: u64, len: usize) -> std::result::Result<Vec<u8>, GenApiError> {
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

    fn write(&self, addr: u64, data: &[u8]) -> std::result::Result<(), GenApiError> {
        self.regs.borrow_mut().insert(addr, data.to_vec());
        Ok(())
    }
}
