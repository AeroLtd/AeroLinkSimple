//! 节点通信编码解码器
//! 实现与ESP32节点的串口通信协议

use crate::node_packets::*;

const FRAME_DELIMITER: u8 = 0x7E;
const MAX_PACKET_SIZE: usize = 1024 * 64;  // 64KB最大包大小

/// 解码状态机状态
#[derive(Debug, Clone, Copy, PartialEq)]
enum DecodeState {
    WaitStart,      // 等待起始标志
    ReadType,       // 读取包类型
    ReadLength,     // 读取长度（4字节）
    ReadData,       // 读取数据
    WaitEnd,        // 等待结束标志
}

/// 节点数据包解码器
#[derive(Debug, Clone)]
pub struct NodeDecoder {
    state: DecodeState,
    buffer: Vec<u8>,
    current_packet_type: Option<PacketType>,
    current_length: u32,
    length_bytes_read: usize,
    length_buffer: [u8; 4],
}

impl NodeDecoder {
    /// 创建新的解码器
    pub fn new() -> Self {
        Self {
            state: DecodeState::WaitStart,
            buffer: Vec::new(),
            current_packet_type: None,
            current_length: 0,
            length_bytes_read: 0,
            length_buffer: [0; 4],
        }
    }

    /// 重置解码器状态
    fn reset(&mut self) {
        self.state = DecodeState::WaitStart;
        self.buffer.clear();
        self.current_packet_type = None;
        self.current_length = 0;
        self.length_bytes_read = 0;
        self.length_buffer = [0; 4];
    }

    /// 解码串口接收到的数据
    /// 返回解码出的完整数据包列表
    pub fn decode(&mut self, data: &[u8]) -> Vec<DecodedPacket> {
        let mut decoded_packets = Vec::new();

        for &byte in data {
            match self.state {
                DecodeState::WaitStart => {
                    if byte == FRAME_DELIMITER {
                        self.state = DecodeState::ReadType;
                        self.buffer.clear();
                        self.length_bytes_read = 0;
                    }
                }

                DecodeState::ReadType => {
                    if byte == FRAME_DELIMITER {
                        // 连续的起始标志，重新开始
                        self.reset();
                        self.state = DecodeState::ReadType;
                    } else if let Ok(packet_type) = PacketType::try_from(byte) {
                        self.current_packet_type = Some(packet_type);
                        self.state = DecodeState::ReadLength;
                        self.current_length = 0;
                        self.length_bytes_read = 0;
                    } else {
                        // 无效的包类型，重置
                        self.reset();
                    }
                }

                DecodeState::ReadLength => {
                    if byte == FRAME_DELIMITER {
                        // 意外的起始标志，重新开始
                        self.reset();
                        self.state = DecodeState::ReadType;
                    } else {
                        self.length_buffer[self.length_bytes_read] = byte;
                        self.length_bytes_read += 1;

                        if self.length_bytes_read == 4 {
                            // 小端序读取32位长度
                            self.current_length = u32::from_le_bytes(self.length_buffer);

                            if self.current_length as usize > MAX_PACKET_SIZE {
                                // 包太大，重置
                                self.reset();
                            } else if self.current_length == 0 {
                                // 空数据包，等待结束标志
                                self.state = DecodeState::WaitEnd;
                            } else {
                                self.state = DecodeState::ReadData;
                                self.buffer.clear();
                                self.buffer.reserve(self.current_length as usize);
                            }
                        }
                    }
                }

                DecodeState::ReadData => {
                    if byte == FRAME_DELIMITER && self.buffer.len() == self.current_length as usize {
                        // 数据接收完成，这应该是结束标志
                        if let Some(packet) = self.parse_packet() {
                            decoded_packets.push(packet);
                        }
                        self.reset();
                    } else if byte == FRAME_DELIMITER && self.buffer.len() < self.current_length as usize {
                        // 数据中出现了起始标志，可能是新包的开始
                        self.reset();
                        self.state = DecodeState::ReadType;
                    } else {
                        self.buffer.push(byte);
                        if self.buffer.len() == self.current_length as usize {
                            self.state = DecodeState::WaitEnd;
                        } else if self.buffer.len() > self.current_length as usize {
                            // 数据超出预期长度，重置
                            self.reset();
                        }
                    }
                }

                DecodeState::WaitEnd => {
                    if byte == FRAME_DELIMITER {
                        // 成功接收到结束标志
                        if let Some(packet) = self.parse_packet() {
                            decoded_packets.push(packet);
                        }
                        self.reset();
                    } else {
                        // 没有正确的结束标志，丢弃当前包
                        self.reset();
                        // 检查这个字节是否是新包的开始
                        if byte == FRAME_DELIMITER {
                            self.state = DecodeState::ReadType;
                        }
                    }
                }
            }
        }

        decoded_packets
    }

    /// 解析缓冲区中的数据为具体的数据包
    fn parse_packet(&self) -> Option<DecodedPacket> {
        let packet_type = self.current_packet_type?;

        match packet_type {
            PacketType::CmdQueryInfo => {
                // 查询命令通常是空载荷
                Some(DecodedPacket::CmdQueryInfo)
            }

            PacketType::CmdSetLoraParam => {
                if self.buffer.len() == std::mem::size_of::<SetLoraParamPayload>() {
                    unsafe {
                        let payload = std::ptr::read(self.buffer.as_ptr() as *const SetLoraParamPayload);
                        Some(DecodedPacket::CmdSetLoraParam(payload))
                    }
                } else {
                    None
                }
            }

            PacketType::TxData => {
                if self.buffer.len() >= 4 {
                    let len = i32::from_le_bytes([
                        self.buffer[0],
                        self.buffer[1],
                        self.buffer[2],
                        self.buffer[3],
                    ]);
                    let data = self.buffer[4..].to_vec();
                    Some(DecodedPacket::TxData(TxDataPacket { len, data }))
                } else {
                    None
                }
            }

            PacketType::RxData => {
                if self.buffer.len() >= 16 {
                    let rssi = f32::from_le_bytes([
                        self.buffer[0],
                        self.buffer[1],
                        self.buffer[2],
                        self.buffer[3],
                    ]);
                    let snr = f32::from_le_bytes([
                        self.buffer[4],
                        self.buffer[5],
                        self.buffer[6],
                        self.buffer[7],
                    ]);
                    let len = i32::from_le_bytes([
                        self.buffer[8],
                        self.buffer[9],
                        self.buffer[10],
                        self.buffer[11],
                    ]);
                    let crc_status = i32::from_le_bytes([
                        self.buffer[12],
                        self.buffer[13],
                        self.buffer[14],
                        self.buffer[15],
                    ]);
                    let data = self.buffer[16..].to_vec();
                    Some(DecodedPacket::RxData(RxDataPacket {
                        rssi,
                        snr,
                        len,
                        crc_status,
                        data,
                    }))
                } else {
                    None
                }
            }

            PacketType::RetNodeInfo => {
                if self.buffer.len() == std::mem::size_of::<CmdQueryInfoPayload>() {
                    unsafe {
                        let payload = std::ptr::read(self.buffer.as_ptr() as *const CmdQueryInfoPayload);
                        Some(DecodedPacket::RetNodeInfo(payload))
                    }
                } else {
                    None
                }
            }
        }
    }

    /// 获取当前解码器状态（用于调试）
    pub fn current_state(&self) -> &'static str {
        match self.state {
            DecodeState::WaitStart => "WaitStart",
            DecodeState::ReadType => "ReadType",
            DecodeState::ReadLength => "ReadLength",
            DecodeState::ReadData => "ReadData",
            DecodeState::WaitEnd => "WaitEnd",
        }
    }
}

/// 节点数据包编码器
#[derive(Debug, Clone)]
pub struct NodeEncoder;

impl NodeEncoder {
    /// 创建新的编码器
    pub fn new() -> Self {
        Self
    }

    /// 编码数据包为字节流
    pub fn encode(&self, packet: &DecodedPacket) -> Vec<u8> {
        let mut result = Vec::new();

        // 起始标志
        result.push(FRAME_DELIMITER);

        // 包类型
        result.push(packet.packet_type() as u8);

        // 准备载荷数据
        let payload = match packet {
            DecodedPacket::CmdQueryInfo => {
                Vec::new()  // 空载荷
            }

            DecodedPacket::CmdSetLoraParam(payload) => {
                unsafe {
                    let ptr = payload as *const SetLoraParamPayload as *const u8;
                    let slice = std::slice::from_raw_parts(ptr, std::mem::size_of::<SetLoraParamPayload>());
                    slice.to_vec()
                }
            }

            DecodedPacket::TxData(tx_data) => {
                let mut data = Vec::new();
                data.extend_from_slice(&tx_data.len.to_le_bytes());
                data.extend_from_slice(&tx_data.data);
                data
            }

            DecodedPacket::RxData(rx_data) => {
                let mut data = Vec::new();
                data.extend_from_slice(&rx_data.rssi.to_le_bytes());
                data.extend_from_slice(&rx_data.snr.to_le_bytes());
                data.extend_from_slice(&rx_data.len.to_le_bytes());
                data.extend_from_slice(&rx_data.crc_status.to_le_bytes());
                data.extend_from_slice(&rx_data.data);
                data
            }

            DecodedPacket::RetNodeInfo(info) => {
                unsafe {
                    let ptr = info as *const CmdQueryInfoPayload as *const u8;
                    let slice = std::slice::from_raw_parts(ptr, std::mem::size_of::<CmdQueryInfoPayload>());
                    slice.to_vec()
                }
            }
        };

        // 长度（4字节，小端序）
        let length = payload.len() as u32;
        result.extend_from_slice(&length.to_le_bytes());

        // 载荷数据
        result.extend_from_slice(&payload);

        // 结束标志
        result.push(FRAME_DELIMITER);

        result
    }

    /// 编码查询节点信息命令
    pub fn encode_query_info_command(&self) -> Vec<u8> {
        self.encode(&DecodedPacket::CmdQueryInfo)
    }

    /// 编码设置LoRa参数命令
    pub fn encode_set_lora_param(&self, mode: NodeWorkingMode, params: LoraParam) -> Vec<u8> {
        let payload = SetLoraParamPayload {
            node_working_mode: mode,
            params,
        };
        self.encode(&DecodedPacket::CmdSetLoraParam(payload))
    }

    /// 编码发送数据命令
    pub fn encode_tx_data(&self, data: &[u8]) -> Vec<u8> {
        let packet = TxDataPacket {
            len: data.len() as i32,
            data: data.to_vec(),
        };
        self.encode(&DecodedPacket::TxData(packet))
    }
}