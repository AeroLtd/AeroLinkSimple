
use anyhow::{Context, Result};
use AeroLinkSimple::node_codec::{NodeDecoder, NodeEncoder};
use AeroLinkSimple::node_packets::*;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;
use tokio::time::sleep;
use tokio_serial::{SerialPortBuilderExt, SerialStream};
use tracing::{error, info, warn};

// ========== 串口配置 ==========
const TX_NODE_PORT: &str = "/dev/ttyUSB0";  // TX节点串口
const RX_NODE_PORT: &str = "/dev/ttyUSB1";  // RX节点串口
const PC_DATA_PORT: &str = "/dev/ttyUSB2";  // PC数据串口

// ========== 波特率配置 ==========
const NODE_BAUD_RATE: u32 = 921600;  // 节点串口波特率
const PC_BAUD_RATE: u32 = 115200;    // PC串口波特率

// ========== 系统配置 ==========
const BUFFER_SIZE: usize = 4096;           // 读取缓冲区大小
const RETRY_DELAY_MS: u64 = 1000;          // 串口打开失败重试延迟(毫秒)
const MAX_RETRY_ATTEMPTS: u32 = 5;         // 最大重试次数
const NODE_CONFIG_DELAY_MS: u64 = 200;     // 节点配置后等待时间(毫秒)
const READ_TIMEOUT_MS: u64 = 100;          // 串口读取超时(毫秒)

// ========== TX节点LoRa参数 ==========
const TX_FREQUENCY: f32 = 434.0;           // 频率 (MHz)
const TX_BANDWIDTH: f32 = 125.0;           // 带宽 (kHz)
const TX_SPREADING_FACTOR: u8 = 9;         // 扩频因子 (7-12)
const TX_CODING_RATE: u8 = 5;              // 编码率 (5-8)
const TX_SYNC_WORD: u8 = 0x12;             // 同步字
const TX_POWER: i8 = 20;                   // 发射功率 (dBm)
const TX_PREAMBLE_LENGTH: u16 = 8;         // 前导码长度

// ========== RX节点LoRa参数 ==========
const RX_FREQUENCY: f32 = 434.0;           // 频率 (MHz)
const RX_BANDWIDTH: f32 = 125.0;           // 带宽 (kHz)
const RX_SPREADING_FACTOR: u8 = 9;         // 扩频因子 (7-12)
const RX_CODING_RATE: u8 = 5;              // 编码率 (5-8)
const RX_SYNC_WORD: u8 = 0x12;             // 同步字
const RX_POWER: i8 = 20;                   // 发射功率 (dBm)
const RX_PREAMBLE_LENGTH: u16 = 8;         // 前导码长度

// ========== 调试配置 ==========
const ENABLE_DATA_HEX_DUMP: bool = false;  // 是否打印数据的十六进制
const ENABLE_PACKET_DEBUG: bool = false;   // 是否打印详细的数据包信息

// ========== 节点管理器 ==========
struct NodeManager {
    tx_port: Arc<Mutex<SerialStream>>,
    rx_port: Arc<Mutex<SerialStream>>,
    pc_port: Arc<Mutex<SerialStream>>,
    tx_encoder: NodeEncoder,
    rx_encoder: NodeEncoder,
}

impl NodeManager {
    async fn new() -> Result<Self> {
        info!("初始化串口连接...");

        // 打开TX节点串口（带重试）
        let tx_port = Self::open_serial_with_retry(
            TX_NODE_PORT,
            NODE_BAUD_RATE,
            "TX节点"
        ).await?;

        // 打开RX节点串口（带重试）
        let rx_port = Self::open_serial_with_retry(
            RX_NODE_PORT,
            NODE_BAUD_RATE,
            "RX节点"
        ).await?;

        // 打开PC串口（带重试）
        let pc_port = Self::open_serial_with_retry(
            PC_DATA_PORT,
            PC_BAUD_RATE,
            "PC数据"
        ).await?;

        Ok(Self {
            tx_port: Arc::new(Mutex::new(tx_port)),
            rx_port: Arc::new(Mutex::new(rx_port)),
            pc_port: Arc::new(Mutex::new(pc_port)),
            tx_encoder: NodeEncoder::new(),
            rx_encoder: NodeEncoder::new(),
        })
    }

    async fn open_serial_with_retry(
        port_name: &str,
        baud_rate: u32,
        description: &str
    ) -> Result<SerialStream> {
        let mut attempts = 0;

        loop {
            attempts += 1;
            info!("尝试打开{}串口: {} (尝试 {}/{})",
                description, port_name, attempts, MAX_RETRY_ATTEMPTS);

            match tokio_serial::new(port_name, baud_rate)
                .timeout(Duration::from_millis(READ_TIMEOUT_MS))
                .open_native_async()
            {
                Ok(port) => {
                    info!("{}串口已成功打开: {} @ {} bps",
                        description, port_name, baud_rate);
                    return Ok(port);
                }
                Err(e) => {
                    warn!("无法打开{}串口: {}", description, e);

                    if attempts >= MAX_RETRY_ATTEMPTS {
                        return Err(anyhow::anyhow!(
                            "打开{}串口失败，已尝试{}次: {}",
                            description, MAX_RETRY_ATTEMPTS, e
                        ));
                    }

                    info!("{}毫秒后重试...", RETRY_DELAY_MS);
                    sleep(Duration::from_millis(RETRY_DELAY_MS)).await;
                }
            }
        }
    }

    async fn configure_nodes(&self) -> Result<()> {
        info!("开始配置节点参数...");

        // 配置TX节点
        info!("配置TX节点: 频率={}MHz, SF={}, BW={}kHz, Power={}dBm",
            TX_FREQUENCY, TX_SPREADING_FACTOR, TX_BANDWIDTH, TX_POWER);

        let tx_params = LoraParam {
            frequency: TX_FREQUENCY,
            bandwidth: TX_BANDWIDTH,
            spreading_factor: TX_SPREADING_FACTOR,
            coding_rate: TX_CODING_RATE,
            sync_word: TX_SYNC_WORD,
            tx_power: TX_POWER,
            preamble_length: TX_PREAMBLE_LENGTH,
        };

        let tx_config_packet = self.tx_encoder.encode_set_lora_param(
            NodeWorkingMode::Tx,
            tx_params,
        );

        {
            let mut port = self.tx_port.lock().await;
            tokio::io::AsyncWriteExt::write_all(&mut *port, &tx_config_packet)
                .await
                .context("发送TX节点配置失败")?;
        }
        info!("TX节点配置已发送");

        // 等待节点处理配置
        sleep(Duration::from_millis(NODE_CONFIG_DELAY_MS)).await;

        // 配置RX节点
        info!("配置RX节点: 频率={}MHz, SF={}, BW={}kHz, Power={}dBm",
            RX_FREQUENCY, RX_SPREADING_FACTOR, RX_BANDWIDTH, RX_POWER);

        let rx_params = LoraParam {
            frequency: RX_FREQUENCY,
            bandwidth: RX_BANDWIDTH,
            spreading_factor: RX_SPREADING_FACTOR,
            coding_rate: RX_CODING_RATE,
            sync_word: RX_SYNC_WORD,
            tx_power: RX_POWER,
            preamble_length: RX_PREAMBLE_LENGTH,
        };

        let rx_config_packet = self.rx_encoder.encode_set_lora_param(
            NodeWorkingMode::Rx,
            rx_params,
        );

        {
            let mut port = self.rx_port.lock().await;
            tokio::io::AsyncWriteExt::write_all(&mut *port, &rx_config_packet)
                .await
                .context("发送RX节点配置失败")?;
        }
        info!("RX节点配置已发送");

        // 等待节点处理配置
        sleep(Duration::from_millis(NODE_CONFIG_DELAY_MS)).await;

        // 查询节点信息以确认配置
        self.query_nodes_info().await?;

        Ok(())
    }

    async fn query_nodes_info(&self) -> Result<()> {
        info!("查询节点信息...");

        let query_packet = self.tx_encoder.encode_query_info_command();

        // 查询TX节点
        {
            let mut port = self.tx_port.lock().await;
            tokio::io::AsyncWriteExt::write_all(&mut *port, &query_packet)
                .await
                .context("查询TX节点信息失败")?;
        }

        sleep(Duration::from_millis(100)).await;

        // 查询RX节点
        {
            let mut port = self.rx_port.lock().await;
            tokio::io::AsyncWriteExt::write_all(&mut *port, &query_packet)
                .await
                .context("查询RX节点信息失败")?;
        }

        Ok(())
    }

    async fn run(&self) -> Result<()> {
        info!("启动数据转发服务...");

        // 创建解码器
        let tx_decoder = Arc::new(Mutex::new(NodeDecoder::new()));
        let rx_decoder = Arc::new(Mutex::new(NodeDecoder::new()));

        // 启动三个异步任务
        let pc_to_tx_handle = self.start_pc_to_tx_task(tx_decoder.clone());
        let rx_to_pc_handle = self.start_rx_to_pc_task(rx_decoder.clone());
        let tx_response_handle = self.start_tx_response_task(tx_decoder.clone());

        info!("所有任务已启动，系统运行中...");

        // 等待任意任务结束（通常不应该发生）
        tokio::select! {
            result = pc_to_tx_handle => {
                error!("PC->TX任务意外结束: {:?}", result);
            }
            result = rx_to_pc_handle => {
                error!("RX->PC任务意外结束: {:?}", result);
            }
            result = tx_response_handle => {
                error!("TX响应处理任务意外结束: {:?}", result);
            }
        }

        Ok(())
    }

    // PC -> TX节点数据转发任务
    fn start_pc_to_tx_task(&self, _decoder: Arc<Mutex<NodeDecoder>>) -> tokio::task::JoinHandle<()> {
        let pc_port = self.pc_port.clone();
        let tx_port = self.tx_port.clone();
        let encoder = NodeEncoder::new();

        tokio::spawn(async move {
            info!("PC->TX转发任务已启动");
            let mut buffer = vec![0u8; BUFFER_SIZE];
            let mut total_bytes = 0u64;
            let mut packet_count = 0u64;

            loop {
                let n = {
                    let mut port = pc_port.lock().await;
                    match tokio::io::AsyncReadExt::read(&mut *port, &mut buffer).await {
                        Ok(n) if n > 0 => n,
                        Ok(_) => {
                            sleep(Duration::from_millis(10)).await;
                            continue;
                        }
                        Err(e) => {
                            if e.kind() != std::io::ErrorKind::TimedOut {
                                error!("PC串口读取错误: {}", e);
                                sleep(Duration::from_millis(100)).await;
                            }
                            continue;
                        }
                    }
                };

                packet_count += 1;
                total_bytes += n as u64;

                info!("PC->TX: 收到第{}个数据包, {}字节 (总计: {}字节)",
                    packet_count, n, total_bytes);

                if ENABLE_DATA_HEX_DUMP && n <= 64 {
                    info!("数据内容: {:02X?}", &buffer[..n]);
                }

                // 将数据封装为TX数据包
                let tx_packet = encoder.encode_tx_data(&buffer[..n]);

                // 发送到TX节点
                let mut port = tx_port.lock().await;
                match tokio::io::AsyncWriteExt::write_all(&mut *port, &tx_packet).await {
                    Ok(_) => {
                        info!("PC->TX: 成功转发{}字节到TX节点", n);
                    }
                    Err(e) => {
                        error!("PC->TX: 发送失败: {}", e);
                    }
                }
            }
        })
    }

    // RX节点 -> PC数据转发任务
    fn start_rx_to_pc_task(&self, decoder: Arc<Mutex<NodeDecoder>>) -> tokio::task::JoinHandle<()> {
        let rx_port = self.rx_port.clone();
        let pc_port = self.pc_port.clone();

        tokio::spawn(async move {
            info!("RX->PC转发任务已启动");
            let mut buffer = vec![0u8; BUFFER_SIZE];
            let mut total_bytes = 0u64;
            let mut packet_count = 0u64;
            let mut error_count = 0u64;

            loop {
                let n = {
                    let mut port = rx_port.lock().await;
                    match tokio::io::AsyncReadExt::read(&mut *port, &mut buffer).await {
                        Ok(n) if n > 0 => n,
                        Ok(_) => {
                            sleep(Duration::from_millis(10)).await;
                            continue;
                        }
                        Err(e) => {
                            if e.kind() != std::io::ErrorKind::TimedOut {
                                error!("RX串口读取错误: {}", e);
                                sleep(Duration::from_millis(100)).await;
                            }
                            continue;
                        }
                    }
                };

                // 解码数据包
                let packets = {
                    let mut dec = decoder.lock().await;
                    dec.decode(&buffer[..n])
                };

                for packet in packets {
                    match packet {
                        DecodedPacket::RxData(rx_data) => {
                            packet_count += 1;

                            let crc_status = if rx_data.crc_status == 1 {
                                "OK"
                            } else {
                                error_count += 1;
                                "ERROR"
                            };

                            info!("RX->PC: 收到第{}个LoRa数据包", packet_count);
                            info!("  长度: {} bytes", rx_data.len);
                            info!("  RSSI: {:.1} dBm", rx_data.rssi);
                            info!("  SNR: {:.1} dB", rx_data.snr);
                            info!("  CRC: {} (错误数: {})", crc_status, error_count);

                            if ENABLE_DATA_HEX_DUMP && rx_data.data.len() <= 64 {
                                info!("  数据: {:02X?}", rx_data.data);
                            }

                            // 只转发CRC正确的数据到PC
                            if rx_data.crc_status == 1 && !rx_data.data.is_empty() {
                                total_bytes += rx_data.data.len() as u64;

                                let mut port = pc_port.lock().await;
                                match tokio::io::AsyncWriteExt::write_all(&mut *port, &rx_data.data).await {
                                    Ok(_) => {
                                        info!("RX->PC: 成功转发{}字节到PC (总计: {}字节)",
                                            rx_data.data.len(), total_bytes);
                                    }
                                    Err(e) => {
                                        error!("RX->PC: 发送失败: {}", e);
                                    }
                                }
                            } else if rx_data.crc_status != 1 {
                                warn!("RX->PC: 丢弃CRC错误的数据包");
                            }
                        }

                        DecodedPacket::RetNodeInfo(info) => {
                            let frequency = info.node_info.frequency;
                            let bandwidth = info.node_info.bandwidth;
                            info!("RX节点信息更新:");
                            info!("  节点ID: {:02X?}", info.node_id);
                            info!("  工作模式: {:?}", info.node_working_mode);
                            info!("  频率: {:.3} MHz", frequency);
                            info!("  带宽: {:.1} kHz", bandwidth);
                            info!("  扩频因子: {}", info.node_info.spreading_factor);
                            info!("  编码率: 4/{}", info.node_info.coding_rate);
                            info!("  发射功率: {} dBm", info.node_info.tx_power);
                        }

                        _ => {
                            if ENABLE_PACKET_DEBUG {
                                warn!("RX节点收到意外的数据包类型: {:?}", packet.packet_type());
                            }
                        }
                    }
                }
            }
        })
    }

    // TX节点响应处理任务（主要用于监控TX节点状态）
    fn start_tx_response_task(&self, decoder: Arc<Mutex<NodeDecoder>>) -> tokio::task::JoinHandle<()> {
        let tx_port = self.tx_port.clone();

        tokio::spawn(async move {
            info!("TX监控任务已启动");
            let mut buffer = vec![0u8; BUFFER_SIZE];

            loop {
                let n = {
                    let mut port = tx_port.lock().await;
                    match tokio::io::AsyncReadExt::read(&mut *port, &mut buffer).await {
                        Ok(n) if n > 0 => n,
                        Ok(_) => {
                            sleep(Duration::from_millis(10)).await;
                            continue;
                        }
                        Err(e) => {
                            if e.kind() != std::io::ErrorKind::TimedOut {
                                error!("TX串口读取错误: {}", e);
                                sleep(Duration::from_millis(100)).await;
                            }
                            continue;
                        }
                    }
                };

                // 解码数据包
                let packets = {
                    let mut dec = decoder.lock().await;
                    dec.decode(&buffer[..n])
                };

                for packet in packets {
                    match packet {
                        DecodedPacket::RetNodeInfo(info) => {
                            info!("TX节点信息更新:");
                            info!("  节点ID: {:02X?}", info.node_id);
                            info!("  工作模式: {:?}", info.node_working_mode);
                            let frequency = info.node_info.frequency;
                            let bandwidth = info.node_info.bandwidth;
                            info!("  频率: {:.3} MHz", frequency);
                            info!("  带宽: {:.1} kHz", bandwidth);
                            info!("  扩频因子: {}", info.node_info.spreading_factor);
                            info!("  编码率: 4/{}", info.node_info.coding_rate);
                            info!("  发射功率: {} dBm", info.node_info.tx_power);
                        }
                        _ => {
                            if ENABLE_PACKET_DEBUG {
                                warn!("TX节点收到意外的数据包类型: {:?}", packet.packet_type());
                            }
                        }
                    }
                }
            }
        })
    }
}

// ========== 主程序 ==========
#[tokio::main]
async fn main() -> Result<()> {
    // 初始化日志系统
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .with_target(false)
        .with_thread_ids(false)
        .with_file(true)
        .with_line_number(true)
        .init();

    info!("====================================");
    info!("    LoRa Bridge System v1.0");
    info!("====================================");
    info!("");
    info!("系统配置:");
    info!("  TX节点串口: {} @ {} bps", TX_NODE_PORT, NODE_BAUD_RATE);
    info!("  RX节点串口: {} @ {} bps", RX_NODE_PORT, NODE_BAUD_RATE);
    info!("  PC数据串口: {} @ {} bps", PC_DATA_PORT, PC_BAUD_RATE);
    info!("");
    info!("TX节点LoRa参数:");
    info!("  频率: {} MHz", TX_FREQUENCY);
    info!("  带宽: {} kHz", TX_BANDWIDTH);
    info!("  扩频因子: {}", TX_SPREADING_FACTOR);
    info!("  编码率: 4/{}", TX_CODING_RATE);
    info!("  发射功率: {} dBm", TX_POWER);
    info!("");
    info!("RX节点LoRa参数:");
    info!("  频率: {} MHz", RX_FREQUENCY);
    info!("  带宽: {} kHz", RX_BANDWIDTH);
    info!("  扩频因子: {}", RX_SPREADING_FACTOR);
    info!("  编码率: 4/{}", RX_CODING_RATE);
    info!("");

    // 创建节点管理器
    info!("正在初始化系统...");
    let manager = match NodeManager::new().await {
        Ok(m) => m,
        Err(e) => {
            error!("系统初始化失败: {}", e);
            std::process::exit(1);
        }
    };

    // 配置节点
    if let Err(e) = manager.configure_nodes().await {
        error!("节点配置失败: {}", e);
        error!("系统将继续运行，但可能需要手动配置节点");
    }

    info!("====================================");
    info!("系统已就绪，开始数据转发...");
    info!("按 Ctrl+C 退出");
    info!("====================================");

    // 设置Ctrl+C处理
    let shutdown = tokio::signal::ctrl_c();

    // 运行主循环
    tokio::select! {
        result = manager.run() => {
            error!("管理器意外退出: {:?}", result);
        }
        _ = shutdown => {
            info!("收到退出信号，正在关闭...");
        }
    }

    info!("系统已关闭");
    Ok(())
}