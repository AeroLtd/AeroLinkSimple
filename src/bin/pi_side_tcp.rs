// src/main.rs

use AeroLinkSimple::node_codec::{NodeDecoder, NodeEncoder};
use AeroLinkSimple::node_packets::*;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, broadcast};
use tokio_serial::{SerialPortBuilderExt, SerialStream};

// ==================== 硬编码配置 ====================
const TX_SERIAL_PORT: &str = "/dev/cu.usbserial-3";
const RX_SERIAL_PORT: &str = "/dev/cu.usbserial-4";
const BAUD_RATE: u32 = 921600;
const TCP_ADDR: &str = "0.0.0.0:8888";

// TX节点LoRa参数
const TX_LORA_PARAMS: LoraParam = LoraParam {
    frequency: 444.0,
    bandwidth: 500.0,
    spreading_factor: 5,
    coding_rate: 5,
    sync_word: 0x12,
    tx_power: 20,
    preamble_length: 8,
};

// RX节点LoRa参数
const RX_LORA_PARAMS: LoraParam = LoraParam {
    frequency: 445.0,
    bandwidth: 500.0,
    spreading_factor: 5,
    coding_rate: 5,
    sync_word: 0x12,
    tx_power: 10,
    preamble_length: 8,
};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("Starting LoRa Gateway...");

    // 创建通道
    let (tcp_to_tx, mut tx_from_tcp) = mpsc::channel::<Vec<u8>>(100);
    let (rx_to_tcp, mut tcp_from_rx) = broadcast::channel::<Vec<u8>>(100);  // 使用broadcast

    // TX节点任务
    let tx_task = tokio::spawn(async move {
        let mut serial = tokio_serial::new(TX_SERIAL_PORT, BAUD_RATE)
            .open_native_async()
            .expect("Failed to open TX serial port");

        let encoder = NodeEncoder::new();
        let mut decoder = NodeDecoder::new();
        let mut buf = [0u8; 1024];
        let mut initialized = false;

        // 初始化TX节点
        println!("Initializing TX node...");



        // 设置参数
        let set_param = encoder.encode_set_lora_param(NodeWorkingMode::Tx, TX_LORA_PARAMS);
        serial.write_all(&set_param).await.unwrap();

        // 查询信息
        serial.write_all(&encoder.encode_query_info_command()).await.unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;

        // 等待确认
        loop {
            if let Ok(n) = tokio::time::timeout(Duration::from_secs(2), serial.read(&mut buf)).await {
                if let Ok(n) = n {
                    let packets = decoder.decode(&buf[..n]);
                    for packet in packets {
                        match packet {
                            DecodedPacket::RetNodeInfo(info) => {
                                println!("TX Node ID: {:02x?}", info.node_id);
                                println!("TX Mode: {:?}", info.node_working_mode);
                                if info.node_working_mode == NodeWorkingMode::Tx {
                                    initialized = true;

                                    // print TX node info
                                    let freq = info.node_info.frequency;
                                    let bw = info.node_info.bandwidth;
                                    let sf = info.node_info.spreading_factor;
                                    let cr = info.node_info.coding_rate;
                                    let pw = info.node_info.tx_power;
                                    println!("TX Node LoRa Params: Freq={}MHz, BW={}kHz, SF={}, CR=4/{}, Power={}dBm",
                                             freq, bw, sf, cr, pw);
                                    break;
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }
            if initialized {
                break;
            }
        }

        println!("TX node initialized!");

        // 主循环：从TCP接收数据并发送
        loop {
            tokio::select! {
                Some(data) = tx_from_tcp.recv() => {
                    let tx_packet = encoder.encode_tx_data(&data);
                    if let Err(e) = serial.write_all(&tx_packet).await {
                        eprintln!("TX serial write error: {}", e);
                    }
                }
                result = serial.read(&mut buf) => {
                    if let Ok(n) = result {
                        let packets = decoder.decode(&buf[..n]);
                        for packet in packets {
                            println!("TX node packet: {:?}", packet);
                        }
                    }
                }
            }
        }
    });

    // RX节点任务
    let rx_task = tokio::spawn(async move {
        let mut serial = tokio_serial::new(RX_SERIAL_PORT, BAUD_RATE)
            .open_native_async()
            .expect("Failed to open RX serial port");

        let encoder = NodeEncoder::new();
        let mut decoder = NodeDecoder::new();
        let mut buf = [0u8; 4096];
        let mut initialized = false;

        // 初始化RX节点
        println!("Initializing RX node...");


        // 设置参数
        let set_param = encoder.encode_set_lora_param(NodeWorkingMode::Rx, RX_LORA_PARAMS);
        serial.write_all(&set_param).await.unwrap();

        // 查询信息
        serial.write_all(&encoder.encode_query_info_command()).await.unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        // 等待确认
        loop {
            if let Ok(n) = tokio::time::timeout(Duration::from_secs(2), serial.read(&mut buf)).await {
                if let Ok(n) = n {
                    let packets = decoder.decode(&buf[..n]);
                    for packet in packets {
                        match packet {
                            DecodedPacket::RetNodeInfo(info) => {
                                println!("RX Node ID: {:02x?}", info.node_id);
                                println!("RX Mode: {:?}", info.node_working_mode);
                                if info.node_working_mode == NodeWorkingMode::Rx {
                                    initialized = true;
                                    // print RX node info
                                    let freq = info.node_info.frequency;
                                    let bw = info.node_info.bandwidth;
                                    let sf = info.node_info.spreading_factor;
                                    let cr = info.node_info.coding_rate;
                                    let pw = info.node_info.tx_power;
                                    println!("RX Node LoRa Params: Freq={}MHz, BW={}kHz, SF={}, CR=4/{}, Power={}dBm",
                                             freq, bw, sf, cr, pw);
                                    break;
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }
            if initialized {
                break;
            }
        }

        println!("RX node initialized!");

        // 主循环：接收数据并转发到TCP
        loop {
            if let Ok(n) = serial.read(&mut buf).await {
                let packets = decoder.decode(&buf[..n]);
                for packet in packets {
                    if let DecodedPacket::RxData(rx_data) = packet {
                        println!("RX: RSSI={:.1}, SNR={:.1}, len={}, crc={}",
                                 rx_data.rssi, rx_data.snr, rx_data.len, rx_data.crc_status);
                        if rx_data.crc_status == 1 {  // CRC OK
                            // 使用broadcast发送
                            let _ = rx_to_tcp.send(rx_data.data);
                        }
                    }else {
                        println!("RX: Receive data packet is invalid");
                    }
                }
            }
        }
    });

    // TCP服务器任务
    let tcp_task = tokio::spawn(async move {
        let listener = TcpListener::bind(TCP_ADDR).await.expect("Failed to bind TCP");
        println!("TCP server listening on {}", TCP_ADDR);

        loop {
            let (mut socket, addr) = listener.accept().await.unwrap();
            println!("Client connected: {}", addr);

            let tcp_to_tx = tcp_to_tx.clone();
            let mut tcp_from_rx = tcp_from_rx.resubscribe();

            tokio::spawn(async move {
                let mut buf = [0u8; 1024];

                loop {
                    tokio::select! {
                        // 从客户端接收数据
                        result = socket.read(&mut buf) => {
                            match result {
                                Ok(0) => {
                                    println!("Client disconnected: {}", addr);
                                    break;
                                }
                                Ok(n) => {
                                    println!("TCP RX {} bytes from {}", n, addr);
                                    if let Err(e) = tcp_to_tx.send(buf[..n].to_vec()).await {
                                        eprintln!("Channel send error: {}", e);
                                        break;
                                    }
                                }
                                Err(e) => {
                                    eprintln!("TCP read error: {}", e);
                                    break;
                                }
                            }
                        }
                        // 发送数据到客户端
                        Ok(data) = tcp_from_rx.recv() => {
                            println!("TCP TX {} bytes to {}", data.len(), addr);
                            if let Err(e) = socket.write_all(&data).await {
                                eprintln!("TCP write error: {}", e);
                                break;
                            }
                        }
                    }
                }
            });
        }
    });

    // 等待所有任务
    let _ = tokio::join!(tx_task, rx_task, tcp_task);

    Ok(())
}