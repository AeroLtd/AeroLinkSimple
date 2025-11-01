use crate::node_codec::{NodeDecoder, NodeEncoder};
use crate::node_packets::*;
use crate::NodeConfig;
use anyhow::Result;
use bytes::BytesMut;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_serial::{SerialPortBuilderExt, SerialStream};

pub struct SerialManager {
    tx_port: Option<SerialStream>,
    rx_port: Option<SerialStream>,
    data_port: SerialStream,
    tx_decoder: NodeDecoder,
    rx_decoder: NodeDecoder,
    encoder: NodeEncoder,
}

impl SerialManager {
    pub async fn new(
        tx_config: Option<NodeConfig>,
        rx_config: Option<NodeConfig>,
        data_port_name: String,
        data_baud: i32,
    ) -> Result<Self> {
        // 打开TX节点串口
        let tx_port = if let Some(config) = tx_config {
            println!("[TX] Opening port: {}", config.port_name);
            let port = tokio_serial::new(&config.port_name, 962100)
                .timeout(Duration::from_millis(100))
                .open_native_async()?;
            Some(port)
        } else {
            println!("[TX] No TX node configured");
            None
        };

        // 打开RX节点串口
        let rx_port = if let Some(config) = rx_config {
            println!("[RX] Opening port: {}", config.port_name);
            let port = tokio_serial::new(&config.port_name, 962100)
                .timeout(Duration::from_millis(100))
                .open_native_async()?;
            Some(port)
        } else {
            println!("[RX] No RX node configured");
            None
        };

        // 打开数据串口
        println!("[DATA] Opening port: {} @ {} baud", data_port_name, data_baud);
        let data_port = tokio_serial::new(&data_port_name, data_baud as u32)
            .timeout(Duration::from_millis(100))
            .open_native_async()?;

        Ok(Self {
            tx_port,
            rx_port,
            data_port,
            tx_decoder: NodeDecoder::new(),
            rx_decoder: NodeDecoder::new(),
            encoder: NodeEncoder::new(),
        })
    }

    pub async fn run(&mut self) {
        let mut data_buffer = vec![0u8; 1024];
        let mut tx_buffer = vec![0u8; 1024];
        let mut rx_buffer = vec![0u8; 1024];
        let mut data_accumulator = BytesMut::new();

        println!("\n[SYSTEM] Bridge running, press Ctrl+C to exit\n");

        loop {
            tokio::select! {
                // 从数据串口读取
                result = self.data_port.read(&mut data_buffer) => {
                    match result {
                        Ok(0) => {
                            // 连接断开
                            tokio::time::sleep(Duration::from_millis(10)).await;
                        }
                        Ok(n) => {
                            data_accumulator.extend_from_slice(&data_buffer[..n]);

                            // 分块发送，每块最多255字节
                            while data_accumulator.len() > 0 {
                                let chunk_size = std::cmp::min(255, data_accumulator.len());
                                let chunk = data_accumulator.split_to(chunk_size);

                                if let Some(ref mut tx_port) = self.tx_port {
                                    let packet = self.encoder.encode_tx_data(&chunk);
                                    match tx_port.write_all(&packet).await {
                                        Ok(_) => {
                                            println!("[TX->LoRa] Sent {} bytes", chunk.len());
                                        }
                                        Err(e) => {
                                            eprintln!("[TX ERROR] Failed to send data: {}", e);
                                        }
                                    }
                                } else {
                                    // TX节点未配置，丢弃数据
                                    println!("[TX] No TX node, dropped {} bytes", chunk.len());
                                }
                            }
                        }
                        Err(e) => {
                            // 读取错误，可能是超时
                            if !e.to_string().contains("deadline") && !e.to_string().contains("timeout") {
                                eprintln!("[DATA ERROR] Read error: {}", e);
                            }
                            tokio::time::sleep(Duration::from_millis(10)).await;
                        }
                    }
                }

                // 从TX节点读取（主要是调试信息）
                result = async {
                    if let Some(ref mut tx_port) = self.tx_port {
                        tx_port.read(&mut tx_buffer).await
                    } else {
                        // 返回一个永远pending的future
                        std::future::pending().await
                    }
                } => {
                    match result {
                        Ok(0) => {
                            tokio::time::sleep(Duration::from_millis(10)).await;
                        }
                        Ok(n) => {
                            let packets = self.tx_decoder.decode(&tx_buffer[..n]);
                            for packet in packets {
                                match packet {
                                    DecodedPacket::RetNodeInfo(info) => {
                                        println!("[TX NODE] Info received - ID: {:02X?}", info.node_id);
                                    }
                                    _ => {
                                        println!("[TX NODE] Packet: {:?}", packet.packet_type());
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            if !e.to_string().contains("deadline") && !e.to_string().contains("timeout") {
                                eprintln!("[TX ERROR] Read error: {}", e);
                            }
                            tokio::time::sleep(Duration::from_millis(10)).await;
                        }
                    }
                }

                // 从RX节点读取
                result = async {
                    if let Some(ref mut rx_port) = self.rx_port {
                        rx_port.read(&mut rx_buffer).await
                    } else {
                        // 返回一个永远pending的future
                        std::future::pending().await
                    }
                } => {
                    match result {
                        Ok(0) => {
                            tokio::time::sleep(Duration::from_millis(10)).await;
                        }
                        Ok(n) => {
                            let packets = self.rx_decoder.decode(&rx_buffer[..n]);
                            for packet in packets {
                                match packet {
                                    DecodedPacket::RxData(rx_data) => {
                                        let crc_status = if rx_data.crc_status == 1 { "OK" } else { "ERROR" };
                                        println!("[RX<-LoRa] {} bytes | RSSI: {:.1} dBm | SNR: {:.1} dB | CRC: {}",
                                            rx_data.data.len(),
                                            rx_data.rssi,
                                            rx_data.snr,
                                            crc_status
                                        );

                                        // 只有CRC正确的数据才转发
                                        if rx_data.crc_status == 1 {
                                            match self.data_port.write_all(&rx_data.data).await {
                                                Ok(_) => {
                                                    println!("[DATA<-RX] Forwarded {} bytes", rx_data.data.len());
                                                }
                                                Err(e) => {
                                                    eprintln!("[DATA ERROR] Failed to forward: {}", e);
                                                }
                                            }
                                        } else {
                                            println!("[RX] Dropped packet with CRC error");
                                        }
                                    }
                                    DecodedPacket::RetNodeInfo(info) => {
                                        println!("[RX NODE] Info received - ID: {:02X?}", info.node_id);
                                    }
                                    _ => {
                                        println!("[RX NODE] Packet: {:?}", packet.packet_type());
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            if !e.to_string().contains("deadline") && !e.to_string().contains("timeout") {
                                eprintln!("[RX ERROR] Read error: {}", e);
                            }
                            tokio::time::sleep(Duration::from_millis(10)).await;
                        }
                    }
                }

                // 定期让出CPU，避免忙等待
                _ = tokio::time::sleep(Duration::from_millis(1)) => {
                    continue;
                }
            }
        }
    }
}