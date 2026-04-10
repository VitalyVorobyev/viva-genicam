use std::cell::RefCell;
use std::collections::HashMap;
use std::env;
use std::error::Error;
use std::net::{IpAddr, SocketAddr};
use std::time::Duration;

use viva_genicam::genapi::{GenApiError, NodeMap, RegisterIo};
use viva_genicam::gige::GigeDevice;
use viva_genicam::sfnc;
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
        <RegisterDescription SchemaMajorVersion="1" SchemaMinorVersion="2" SchemaSubMinorVersion="3">
            <Integer Name="Width">
                <Address>0x100</Address>
                <Length>4</Length>
                <AccessMode>RW</AccessMode>
                <Min>16</Min>
                <Max>4096</Max>
                <Inc>2</Inc>
            </Integer>
            <Float Name="ExposureTime">
                <Address>0x200</Address>
                <Length>4</Length>
                <AccessMode>RW</AccessMode>
                <Min>10.0</Min>
                <Max>100000.0</Max>
                <Scale>1/1000</Scale>
            </Float>
            <Enumeration Name="GainSelector">
                <Address>0x300</Address>
                <Length>2</Length>
                <AccessMode>RW</AccessMode>
                <EnumEntry Name="AnalogAll" Value="0" />
                <EnumEntry Name="DigitalAll" Value="1" />
            </Enumeration>
            <Integer Name="Gain">
                <Address>0x304</Address>
                <Length>2</Length>
                <AccessMode>RW</AccessMode>
                <Min>0</Min>
                <Max>48</Max>
                <pSelected>GainSelector</pSelected>
                <Selected>AnalogAll</Selected>
            </Integer>
            <Boolean Name="GammaEnable">
                <Address>0x400</Address>
                <Length>1</Length>
                <AccessMode>RW</AccessMode>
            </Boolean>
            <Command Name="AcquisitionStart">
                <Address>0x500</Address>
                <Length>4</Length>
            </Command>
            <Command Name="AcquisitionStop">
                <Address>0x504</Address>
                <Length>4</Length>
            </Command>
        </RegisterDescription>
    "#;

    let model = viva_genapi_xml::parse(XML)?;
    let nodemap = NodeMap::from(model);
    let transport = MockIo::with_registers(&[
        (0x100, vec![0, 0, 4, 0]),
        (0x200, vec![0, 0, 0xC3, 0x50]),
        (0x300, vec![0, 0]),
        (0x304, vec![0, 20]),
        (0x400, vec![1]),
        (0x500, vec![0, 0, 0, 0]),
        (0x504, vec![0, 0, 0, 0]),
    ]);
    let mut camera = Camera::new(transport, nodemap);

    println!("Mock camera features:");
    println!("  Width -> {}", camera.get("Width")?);
    camera.set("Width", "2048")?;
    println!("  Width (after set) -> {}", camera.get("Width")?);

    println!("  ExposureTime -> {}", camera.get(sfnc::EXPOSURE_TIME)?);
    camera.set_exposure_time_us(75.0)?;
    println!(
        "  ExposureTime (after set) -> {}",
        camera.get(sfnc::EXPOSURE_TIME)?
    );

    println!("  GainSelector -> {}", camera.get(sfnc::GAIN_SELECTOR)?);
    println!("  Gain -> {}", camera.get(sfnc::GAIN)?);

    println!("  GammaEnable -> {}", camera.get("GammaEnable")?);
    camera.set("GammaEnable", "false")?;
    println!(
        "  GammaEnable (after set) -> {}",
        camera.get("GammaEnable")?
    );

    camera.acquisition_start()?;
    camera.acquisition_stop()?;

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

    println!("  ExposureTime -> {}", camera.get(sfnc::EXPOSURE_TIME)?);
    camera.set_exposure_time_us(5000.0)?;
    println!(
        "  ExposureTime (after set) -> {}",
        camera.get(sfnc::EXPOSURE_TIME)?
    );

    if let Ok(value) = camera.get(sfnc::GAIN) {
        println!("  Gain -> {value}");
        camera.set(sfnc::GAIN, value.as_str())?;
    }

    println!("Issuing AcquisitionStart/Stop");
    camera.acquisition_start()?;
    camera.acquisition_stop()?;
    Ok(())
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
