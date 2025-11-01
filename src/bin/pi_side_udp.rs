
use anyhow::{Context, Result};
use AeroLinkSimple::node_codec::{NodeDecoder, NodeEncoder};
use AeroLinkSimple::node_packets::*;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;
use tokio::time::sleep;
use tokio_serial::{SerialPortBuilderExt, SerialStream};
use tracing::{error, info, warn};
use tokio::net::UdpSocket;
use std::net::SocketAddr;
use std::collections::HashMap;

// ========== 串口配置 ==========
const TX_NODE_PORT: &str = "/dev/ttyUSB0";  // TX节点串口
const RX_NODE_PORT: &str = "/dev/ttyUSB1";  // RX节点串口

// ========== UDP配置 ==========
const UDP_BIND_ADDR: &str = "0.0.0.0";      // 绑定地址 (0.0.0.0 表示所有网卡)
const UDP_TX_PORT: u16 = 8888;              // TX数据接收端口（PC发送到此端口的数据会通过LoRa发送）
const UDP_RX_PORT: u16 = 8889;              // RX数据发送端口（LoRa接收的数据从此端口广播）
const UDP_CLIENT_TIMEOUT_SEC: u64 = 30;     // 客户端超时时间（秒）
const UDP_MAX_PACKET_SIZE: usize = 65507;   // UDP最大包大小

// ========== 波特率配置 ==========
const NODE_BAUD_RATE: u32 = 921600;         // 节点串口波特率

// ========== 系统配置 ==========
const BUFFER_SIZE: usize = 4096;            // 读取缓冲区大小
const RETRY_DELAY_MS: u64 = 1000;           // 串口打开失败重试延迟(毫秒)
const MAX_RETRY_ATTEMPTS: u32 = 5;          // 最大重试次数
const NODE_CONFIG_DELAY_MS: u64 = 200;      // 节点配置后等待时间(毫秒)
const READ_TIMEOUT_MS: u64 = 100;           // 串口读取超时(毫秒)
const CLIENT_CLEANUP_INTERVAL_SEC: u64 = 5; // 客户端清理间隔（秒）

// ========== TX节点LoRa参数 ==========
const TX_FREQUENCY: f32 = 434.0;            // 频率 (MHz)
const TX_BANDWIDTH: f32 = 125.0;            // 带宽 (kHz)
const TX_SPREADING_FACTOR: u8 = 9;          // 扩频因子 (7-12)
const TX_CODING_RATE: u8 = 5;               // 编码率 (5-8)
const TX_SYNC_WORD: u8 = 0x12;              // 同步字
const TX_POWER: i8 = 20;                    // 发射功率 (dBm)
const TX_PREAMBLE_LENGTH: u16 = 8;          // 前导码长度

// ========== RX节点LoRa参数 ==========
const RX_FREQUENCY: f32 = 434.0;            // 频率 (MHz)
const RX_BANDWIDTH: f32 = 125.0;            // 带宽 (kHz)
const RX_SPREADING_FACTOR: u8 = 9;          // 扩频因子 (7-12)
const RX_CODING_RATE: u8 = 5;               // 编码率 (5-8)
const RX_SYNC_WORD: u8 = 0x12;              // 同步字
const RX_POWER: i8 = 20;                    // 发射功率 (dBm)
const RX_PREAMBLE_LENGTH: u16 = 8;          // 前导码长度

// ========== 调试配置 ==========
const ENABLE_DATA_HEX_DUMP: bool = false;   // 是否打印数据的十六进制
const ENABLE_PACKET_DEBUG: bool = false;    // 是否打印详细的数据包信息

// ========== UDP客户端信息 ==========
#[derive(Clone, Debug)]
struct UdpClientInfo {
    last_seen: std::time::Instant,
    packet_count: u64,
    byte_count: u64,
}

// ========== UDP客户端管理器 ==========
struct UdpClientManager {
    tx_clients: Arc<Mutex<HashMap<SocketAddr, UdpClientInfo>>>,  // 发送数据的客户端
    rx_clients: Arc<Mutex<HashMap<SocketAddr, UdpClientInfo>>>,  // 接收数据的客户端
}

impl UdpClientManager {
    fn new() -> Self {
        Self {
            tx_clients: Arc::new(Mutex::new(HashMap::new())),
            rx_clients: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    async fn update_tx_client(&self, addr: SocketAddr) {
        let mut clients = self.tx_clients.lock().await;
        let client = clients.entry(addr).or_insert(UdpClientInfo {
            last_seen: std::time::Instant::now(),
            packet_count: 0,
            byte_count: 0,
        });
        client.last_seen = std::time::Instant::now();
        client.packet_count += 1;
    }

    async fn register_rx_client(&self, addr: SocketAddr) {
        let mut clients = self.rx_clients.lock().await;
        if !clients.contains_key(&addr) {
            info!("新的RX客户端注册: {}", addr);
            clients.insert(addr, UdpClientInfo {
                last_seen: std::time::Instant::now(),
                packet_count: 0,
                byte_count: 0,
            });
        } else {
            clients.get_mut(&addr).unwrap().last_seen = std::time::Instant::now();
        }
    }

    async fn get_rx_clients(&self) -> Vec<SocketAddr> {
        let clients = self.rx_clients.lock().await;
        clients.keys().cloned().collect()
    }

    async fn cleanup_inactive_clients(&self) {
        let now = std::time::Instant::now();
        let timeout = Duration::from_secs(UDP_CLIENT_TIMEOUT_SEC);

        // 清理TX客户端
        {
            let mut clients = self.tx_clients.lock().await;
            let before_count = clients.len();
            clients.retain(|addr, info| {
                let is_active = now.duration_since(info.last_seen) < timeout;
                if !is_active {
                    info!("移除不活跃的TX客户端: {} ({}个包, {}字节)",
                        addr, info.packet_count, info.byte_count);
                }
                is_active
            });
            if before_count != clients.len() {
                info!("TX客户端数: {} -> {}", before_count, clients.len());
            }
        }

        // 清理RX客户端
        {
            let mut clients = self.rx_clients.lock().await;
            let before_count = clients.len();
            clients.retain(|addr, info| {
                let is_active = now.duration_since(info.last_seen) < timeout;
                if !is_active {
                    info!("移除不活跃的RX客户端: {} ({}个包, {}字节)",
                        addr, info.packet_count, info.byte_count);
                }
                is_active
            });
            if before_count != clients.len() {
                info!("RX客户端数: {} -> {}", before_count, clients.len());
            }
        }
    }

    async fn update_rx_stats(&self, addr: &SocketAddr, bytes: usize) {
        let mut clients = self.rx_clients.lock().await;
        if let Some(client) = clients.get_mut(addr) {
            client.packet_count += 1;
            client.byte_count += bytes as u64;
        }
    }

    async fn get_stats(&self) -> (usize, usize) {
        let tx_count = self.tx_clients.lock().await.len();
        let rx_count = self.rx_clients.lock().await.len();
        (tx_count, rx_count)
    }
}

// ========== 节点管理器 ==========
struct NodeManager {
    tx_port: Arc<Mutex<SerialStream>>,
    rx_port: Arc<Mutex<SerialStream>>,
    udp_tx_socket: Arc<UdpSocket>,
    udp_rx_socket: Arc<UdpSocket>,
    udp_clients: Arc<UdpClientManager>,
    tx_encoder: NodeEncoder,
    rx_encoder: NodeEncoder,
}

impl NodeManager {
    async fn new() -> Result<Self> {
        info!("初始化系统...");

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

        // 创建UDP sockets
        let tx_addr = format!("{}:{}", UDP_BIND_ADDR, UDP_TX_PORT);
        let rx_addr = format!("{}:{}", UDP_BIND_ADDR, UDP_RX_PORT);

        info!("绑定UDP TX端口: {}", tx_addr);
        let udp_tx_socket = UdpSocket::bind(&tx_addr)
            .await
            .context(format!("无法绑定UDP TX端口: {}", tx_addr))?;

        info!("绑定UDP RX端口: {}", rx_addr);
        let udp_rx_socket = UdpSocket::bind(&rx_addr)
            .await
            .context(format!("无法绑定UDP RX端口: {}", rx_addr))?;

        Ok(Self {
            tx_port: Arc::new(Mutex::new(tx_port)),
            rx_port: Arc::new(Mutex::new(rx_port)),
            udp_tx_socket: Arc::new(udp_tx_socket),
            udp_rx_socket: Arc::new(udp_rx_socket),
            udp_clients: Arc::new(UdpClientManager::new()),
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

        // 启动各个任务
        let udp_to_tx_handle = self.start_udp_to_tx_task();
        let rx_to_udp_handle = self.start_rx_to_udp_task(rx_decoder.clone());
        let tx_response_handle = self.start_tx_response_task(tx_decoder.clone());
        let client_cleanup_handle = self.start_client_cleanup_task();
        let udp_register_handle = self.start_udp_register_task();

        info!("所有任务已启动，系统运行中...");
        info!("");
        info!("使用方法:");
        info!("  发送数据: 将UDP数据包发送到 {}:{}", UDP_BIND_ADDR, UDP_TX_PORT);
        info!("  接收数据: 先发送任意数据到 {}:{} 进行注册", UDP_BIND_ADDR, UDP_RX_PORT);
        info!("           然后从相同端口接收数据");
        info!("");

        // 等待任意任务结束（通常不应该发生）
        tokio::select! {
            result = udp_to_tx_handle => {
                error!("UDP->TX任务意外结束: {:?}", result);
            }
            result = rx_to_udp_handle => {
                error!("RX->UDP任务意外结束: {:?}", result);
            }
            result = tx_response_handle => {
                error!("TX响应处理任务意外结束: {:?}", result);
            }
            result = client_cleanup_handle => {
                error!("客户端清理任务意外结束: {:?}", result);
            }
            result = udp_register_handle => {
                error!("UDP注册任务意外结束: {:?}", result);
            }
        }

        Ok(())
    }

    // UDP -> TX节点数据转发任务
    fn start_udp_to_tx_task(&self) -> tokio::task::JoinHandle<()> {
        let udp_socket = self.udp_tx_socket.clone();
        let tx_port = self.tx_port.clone();
        let encoder = self.tx_encoder.clone();
        let clients = self.udp_clients.clone();

        tokio::spawn(async move {
            info!("UDP->TX转发任务已启动 (端口: {})", UDP_TX_PORT);
            let mut buffer = vec![0u8; UDP_MAX_PACKET_SIZE];
            let mut total_bytes = 0u64;
            let mut packet_count = 0u64;

            loop {
                match udp_socket.recv_from(&mut buffer).await {
                    Ok((n, addr)) => {
                        packet_count += 1;
                        total_bytes += n as u64;

                        // 更新客户端信息
                        clients.update_tx_client(addr).await;

                        info!("UDP->TX [{}]: 收到第{}个数据包, {}字节 (总计: {}字节)",
                            addr, packet_count, n, total_bytes);

                        if ENABLE_DATA_HEX_DUMP && n <= 64 {
                            info!("数据内容: {:02X?}", &buffer[..n]);
                        }

                        // 将数据封装为TX数据包
                        let tx_packet = encoder.encode_tx_data(&buffer[..n]);

                        // 发送到TX节点
                        let mut port = tx_port.lock().await;
                        match tokio::io::AsyncWriteExt::write_all(&mut *port, &tx_packet).await {
                            Ok(_) => {
                                info!("UDP->TX [{}]: 成功转发{}字节到TX节点", addr, n);
                            }
                            Err(e) => {
                                error!("UDP->TX [{}]: 发送失败: {}", addr, e);
                            }
                        }
                    }
                    Err(e) => {
                        error!("UDP接收错误: {}", e);
                        sleep(Duration::from_millis(100)).await;
                    }
                }
            }
        })
    }

    // RX节点 -> UDP数据转发任务
    fn start_rx_to_udp_task(&self, decoder: Arc<Mutex<NodeDecoder>>) -> tokio::task::JoinHandle<()> {
        let rx_port = self.rx_port.clone();
        let udp_socket = self.udp_rx_socket.clone();
        let clients = self.udp_clients.clone();

        tokio::spawn(async move {
            info!("RX->UDP转发任务已启动 (端口: {})", UDP_RX_PORT);
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

                            info!("RX->UDP: 收到第{}个LoRa数据包", packet_count);
                            info!("  长度: {} bytes", rx_data.len);
                            info!("  RSSI: {:.1} dBm", rx_data.rssi);
                            info!("  SNR: {:.1} dB", rx_data.snr);
                            info!("  CRC: {} (错误数: {})", crc_status, error_count);

                            if ENABLE_DATA_HEX_DUMP && rx_data.data.len() <= 64 {
                                info!("  数据: {:02X?}", rx_data.data);
                            }

                            // 只转发CRC正确的数据到UDP客户端
                            if rx_data.crc_status == 1 && !rx_data.data.is_empty() {
                                total_bytes += rx_data.data.len() as u64;

                                // 获取所有注册的RX客户端
                                let rx_clients = clients.get_rx_clients().await;

                                if rx_clients.is_empty() {
                                    warn!("RX->UDP: 没有注册的客户端接收数据");
                                } else {
                                    let mut success_count = 0;
                                    for client_addr in &rx_clients {
                                        match udp_socket.send_to(&rx_data.data, client_addr).await {
                                            Ok(_) => {
                                                success_count += 1;
                                                clients.update_rx_stats(client_addr, rx_data.data.len()).await;
                                            }
                                            Err(e) => {
                                                warn!("发送到客户端 {} 失败: {}", client_addr, e);
                                            }
                                        }
                                    }

                                    info!("RX->UDP: 成功转发{}字节到{}个客户端 (总计: {}字节)",
                                        rx_data.data.len(), success_count, total_bytes);
                                }
                            } else if rx_data.crc_status != 1 {
                                warn!("RX->UDP: 丢弃CRC错误的数据包");
                            }
                        }

                        DecodedPacket::RetNodeInfo(info) => {
                            info!("RX节点信息更新:");
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
                                warn!("RX节点收到意外的数据包类型: {:?}", packet.packet_type());
                            }
                        }
                    }
                }
            }
        })
    }

    // TX节点响应处理任务
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

    // UDP客户端注册任务（接收注册请求）
    fn start_udp_register_task(&self) -> tokio::task::JoinHandle<()> {
        let udp_socket = self.udp_rx_socket.clone();
        let clients = self.udp_clients.clone();

        tokio::spawn(async move {
            info!("UDP客户端注册任务已启动");
            let mut buffer = vec![0u8; 256];

            loop {
                // 监听注册端口上的任何数据
                match udp_socket.recv_from(&mut buffer).await {
                    Ok((n, addr)) => {
                        // 收到数据就注册客户端
                        clients.register_rx_client(addr).await;

                        // 如果收到"REGISTER"命令，发送确认
                        if n >= 8 && &buffer[..8] == b"REGISTER" {
                            info!("收到客户端 {} 的注册请求", addr);
                            let response = b"REGISTERED";
                            if let Err(e) = udp_socket.send_to(response, addr).await {
                                warn!("发送注册确认失败: {}", e);
                            }
                        }
                    }
                    Err(e) => {
                        // 这个错误是正常的，因为我们在同一个socket上既接收注册又发送数据
                        if e.kind() != std::io::ErrorKind::WouldBlock {
                            // 只记录非阻塞错误
                        }
                        sleep(Duration::from_millis(10)).await;
                    }
                }
            }
        })
    }

    // 客户端清理任务
    fn start_client_cleanup_task(&self) -> tokio::task::JoinHandle<()> {
        let clients = self.udp_clients.clone();

        tokio::spawn(async move {
            info!("客户端清理任务已启动");

            loop {
                sleep(Duration::from_secs(CLIENT_CLEANUP_INTERVAL_SEC)).await;

                clients.cleanup_inactive_clients().await;

                let (tx_count, rx_count) = clients.get_stats().await;
                if tx_count > 0 || rx_count > 0 {
                    info!("活跃客户端统计 - TX: {}, RX: {}", tx_count, rx_count);
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
    info!("    LoRa Bridge System v1.0 (UDP)");
    info!("====================================");
    info!("");
    info!("系统配置:");
    info!("  TX节点串口: {} @ {} bps", TX_NODE_PORT, NODE_BAUD_RATE);
    info!("  RX节点串口: {} @ {} bps", RX_NODE_PORT, NODE_BAUD_RATE);
    info!("  UDP TX端口: {} (PC->LoRa)", UDP_TX_PORT);
    info!("  UDP RX端口: {} (LoRa->PC)", UDP_RX_PORT);
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
    info!("系统已就绪");
    info!("");
    info!("使用说明:");
    info!("1. 发送数据: 将UDP包发送到端口 {}", UDP_TX_PORT);
    info!("2. 接收数据: ");
    info!("   a) 先发送'REGISTER'到端口 {} 进行注册", UDP_RX_PORT);
    info!("   b) 然后在同一端口接收数据");
    info!("");
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