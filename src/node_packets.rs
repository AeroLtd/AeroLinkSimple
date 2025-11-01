//! 节点通信数据包定义
//! 对应ESP32节点的Packets.h

/// 数据包类型
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PacketType {
    CmdQueryInfo = 0x01,
    CmdSetLoraParam = 0x02,
    TxData = 0x03,
    RxData = 0x04,
    RetNodeInfo = 0x05,
}

impl TryFrom<u8> for PacketType {
    type Error = ();

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0x01 => Ok(PacketType::CmdQueryInfo),
            0x02 => Ok(PacketType::CmdSetLoraParam),
            0x03 => Ok(PacketType::TxData),
            0x04 => Ok(PacketType::RxData),
            0x05 => Ok(PacketType::RetNodeInfo),
            _ => Err(()),
        }
    }
}

/// 节点工作模式
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeWorkingMode {
    Tx = 0x01,
    Rx = 0x02,
    NotSet = 0xFF,
}

impl TryFrom<u8> for NodeWorkingMode {
    type Error = ();

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0x01 => Ok(NodeWorkingMode::Tx),
            0x02 => Ok(NodeWorkingMode::Rx),
            0xFF => Ok(NodeWorkingMode::NotSet),
            _ => Err(()),
        }
    }
}

/// LoRa参数结构体（14字节）
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct LoraParam {
    pub frequency: f32,           // 4 bytes
    pub bandwidth: f32,           // 4 bytes
    pub spreading_factor: u8,    // 1 byte
    pub coding_rate: u8,          // 1 byte
    pub sync_word: u8,            // 1 byte (0x12)
    pub tx_power: i8,             // 1 byte
    pub preamble_length: u16,    // 2 bytes
}

impl Default for LoraParam {
    fn default() -> Self {
        Self {
            frequency: 434.0,
            bandwidth: 125.0,
            spreading_factor: 9,
            coding_rate: 5,
            sync_word: 0x12,
            tx_power: 10,
            preamble_length: 8,
        }
    }
}

/// 设置LoRa参数命令载荷（15字节）
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct SetLoraParamPayload {
    pub node_working_mode: NodeWorkingMode,  // 1 byte
    pub params: LoraParam,                   // 14 bytes
}

/// 查询节点信息返回载荷（21字节）
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct CmdQueryInfoPayload {
    pub node_id: [u8; 6],                    // 6 bytes
    pub node_working_mode: NodeWorkingMode,  // 1 byte
    pub node_info: LoraParam,                // 14 bytes
}

/// TX数据包
#[derive(Debug, Clone)]
pub struct TxDataPacket {
    pub len: i32,           // 4 bytes
    pub data: Vec<u8>,      // variable length
}

/// RX数据包
#[derive(Debug, Clone)]
pub struct RxDataPacket {
    pub rssi: f32,          // 4 bytes
    pub snr: f32,           // 4 bytes
    pub len: i32,           // 4 bytes
    pub crc_status: i32,    // 4 bytes (1=OK, 0=ERROR)
    pub data: Vec<u8>,      // variable length
}

/// 解码后的数据包
#[derive(Debug, Clone)]
pub enum DecodedPacket {
    CmdQueryInfo,  // 空载荷
    CmdSetLoraParam(SetLoraParamPayload),
    TxData(TxDataPacket),
    RxData(RxDataPacket),
    RetNodeInfo(CmdQueryInfoPayload),
}

impl DecodedPacket {
    /// 获取数据包类型
    pub fn packet_type(&self) -> PacketType {
        match self {
            DecodedPacket::CmdQueryInfo => PacketType::CmdQueryInfo,
            DecodedPacket::CmdSetLoraParam(_) => PacketType::CmdSetLoraParam,
            DecodedPacket::TxData(_) => PacketType::TxData,
            DecodedPacket::RxData(_) => PacketType::RxData,
            DecodedPacket::RetNodeInfo(_) => PacketType::RetNodeInfo,
        }
    }
}