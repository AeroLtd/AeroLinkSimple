

use anyhow::Result;
use AeroLinkSimple::node_codec::{NodeDecoder, NodeEncoder};
use AeroLinkSimple::node_packets::*;
use AeroLinkSimple::serial_manager::SerialManager;
use serialport::available_ports;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;
use AeroLinkSimple::NodeConfig;
use AeroLinkSimple::ui::UserInterface;


#[tokio::main]
async fn main() -> Result<()> {
    let mut ui = UserInterface::new();

    // 显示欢迎信息
    ui.print_header("LoRa Bridge System - Configuration");

    // 获取可用串口列表
    let ports = available_ports()?;
    if ports.is_empty() {
        ui.print_error("No serial ports found!");
        return Ok(());
    }

    let port_names: Vec<String> = ports.iter().map(|p| p.port_name.clone()).collect();

    // 选择TX节点
    ui.print_section("TX Node Configuration");
    let tx_config = configure_node(&mut ui, &port_names, NodeWorkingMode::Tx, "TX").await?;

    // 选择RX节点
    ui.print_section("RX Node Configuration");
    let rx_config = configure_node(&mut ui, &port_names, NodeWorkingMode::Rx, "RX").await?;

    // 选择数据输入串口
    ui.print_section("Data Input Serial Port Configuration");
    let data_port = ui.select_from_list("Select data input port:", &port_names)?;
    let data_baud = ui.input_number("Enter baud rate for data port", 9600, 4000000, 962100)?;

    // 启动通信系统
    ui.print_header("Starting LoRa Bridge System");

    let manager = Arc::new(Mutex::new(
        SerialManager::new(tx_config, rx_config, data_port, data_baud).await?
    ));

    // 启动主循环
    run_bridge_system(manager, ui).await?;

    Ok(())
}

async fn configure_node(
    ui: &mut UserInterface,
    port_names: &[String],
    mode: NodeWorkingMode,
    node_name: &str,
) -> Result<Option<NodeConfig>> {

    // 添加"None"选项
    let mut options = vec!["None (Skip this node)".to_string()];
    options.extend_from_slice(port_names);

    let selection = ui.select_from_list(
        &format!("Select serial port for {} node:", node_name),
        &options,
    )?;

    if selection == "None (Skip this node)" {
        ui.print_warning(&format!("{} node will not be configured", node_name));
        return Ok(None);
    }

    // 尝试查询节点信息
    ui.print_info(&format!("Querying {} node information...", node_name));

    let current_params = match query_node_info(&selection).await {
        Ok(info) => {
            ui.print_success(&format!("Successfully queried {} node", node_name));
            ui.print_info(&format!("Node ID: {:02X?}", info.node_id));
            Some(info.node_info)
        }
        Err(e) => {
            ui.print_warning(&format!("Failed to query node: {}", e));
            None
        }
    };

    // 配置LoRa参数
    ui.print_section(&format!("{} Node LoRa Parameters", node_name));

    let mut params = current_params.unwrap_or_default();

    // 频率设置
    params.frequency = ui.input_number_float(
        "Enter frequency (MHz)",
        200.0,
        980.0,
        params.frequency,
    )?;

    // 带宽选择
    let bandwidths = vec![
        ("7.8 kHz", 7.8),
        ("10.4 kHz", 10.4),
        ("15.6 kHz", 15.6),
        ("20.8 kHz", 20.8),
        ("31.25 kHz", 31.25),
        ("41.7 kHz", 41.7),
        ("62.5 kHz", 62.5),
        ("125 kHz", 125.0),
        ("250 kHz", 250.0),
        ("500 kHz", 500.0),
    ];

    let bw_labels: Vec<String> = bandwidths.iter().map(|(l, _)| l.to_string()).collect();
    let selected_bw = ui.select_from_list("Select bandwidth:", &bw_labels)?;
    params.bandwidth = bandwidths
        .iter()
        .find(|(l, _)| l.to_string() == selected_bw)
        .unwrap()
        .1;

    // SF选择
    params.spreading_factor = ui.select_number("Select Spreading Factor:", 5, 12, params.spreading_factor as i32)? as u8;

    // Coding Rate选择
    params.coding_rate = ui.select_number("Select Coding Rate (4/x):", 5, 8, params.coding_rate as i32)? as u8;

    // TX Power设置
    params.tx_power = ui.select_number("Select TX Power (dBm):", -6, 22, params.tx_power as i32)? as i8;

    // Preamble长度
    params.preamble_length = ui.input_number("Enter preamble length", 6, 65535, params.preamble_length as i32)? as u16;

    // Sync Word
    let sync = ui.input_hex("Enter sync word (hex)", params.sync_word)?;
    params.sync_word = sync;

    // 设置节点参数
    ui.print_info(&format!("Setting {} node parameters...", node_name));

    match set_node_params(&selection, mode, params).await {
        Ok(_) => ui.print_success(&format!("{} node configured successfully", node_name)),
        Err(e) => ui.print_warning(&format!("Failed to set {} node parameters: {}", node_name, e)),
    }

    Ok(Some(NodeConfig {
        port_name: selection,
        params,
        mode,
    }))
}

async fn query_node_info(port_name: &str) -> Result<CmdQueryInfoPayload> {
    use tokio_serial::SerialPortBuilderExt;

    let mut port = tokio_serial::new(port_name, 962100)
        .timeout(Duration::from_millis(100))
        .open_native_async()?;

    let encoder = NodeEncoder::new();
    let mut decoder = NodeDecoder::new();

    // 发送查询命令
    let query_cmd = encoder.encode_query_info_command();
    use tokio::io::AsyncWriteExt;
    port.write_all(&query_cmd).await?;

    // 等待响应
    let timeout = tokio::time::timeout(Duration::from_secs(2), async {
        let mut buffer = vec![0u8; 1024];
        loop {
            use tokio::io::AsyncReadExt;
            match port.read(&mut buffer).await {
                Ok(n) if n > 0 => {
                    let packets = decoder.decode(&buffer[..n]);
                    for packet in packets {
                        if let DecodedPacket::RetNodeInfo(info) = packet {
                            return Ok(info);
                        }
                    }
                }
                _ => {}
            }
        }
    });

    timeout.await?
}

async fn set_node_params(
    port_name: &str,
    mode: NodeWorkingMode,
    params: LoraParam,
) -> Result<()> {
    use tokio_serial::SerialPortBuilderExt;

    let mut port = tokio_serial::new(port_name, 962100)
        .timeout(Duration::from_millis(100))
        .open_native_async()?;

    let encoder = NodeEncoder::new();
    let cmd = encoder.encode_set_lora_param(mode, params);

    use tokio::io::AsyncWriteExt;
    port.write_all(&cmd).await?;

    // 等待一下让设置生效
    tokio::time::sleep(Duration::from_millis(500)).await;

    Ok(())
}

async fn run_bridge_system(
    manager: Arc<Mutex<SerialManager>>,
    mut ui: UserInterface,
) -> Result<()> {
    ui.clear_screen();
    ui.print_header("LoRa Bridge System - Running");
    ui.print_info("Press Ctrl+C to exit\n");

    let manager_clone = manager.clone();

    // 启动串口管理器
    tokio::spawn(async move {
        let mut mgr = manager_clone.lock().await;
        mgr.run().await;
    });

    // 等待Ctrl+C
    tokio::signal::ctrl_c().await?;
    ui.print_info("\nShutting down...");

    Ok(())
}