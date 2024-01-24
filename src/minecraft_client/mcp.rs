#![allow(dead_code)]

use anyhow::{anyhow, Result};
use chrono::Local;
use mc_varint::{VarInt, VarIntRead, VarIntWrite};
use parking_lot::{Mutex, RwLock};
use rand;
use std::thread;
use std::{
    io::{Cursor, Read, Write},
    net::TcpStream,
    sync::{
        mpsc::{Receiver, Sender},
        Arc,
    },
};
use uuid::Uuid;

pub trait McpConnection {}

impl McpConnection for Mcp764Connection {}

pub fn connect(
    port: u16,
    player_name: String,
    player_uuid: Uuid,
    chat_text_sender: Sender<String>,
    chat_text_receiver: Receiver<String>,
) -> Result<Box<dyn McpConnection>> {
    Ok(Box::new(Mcp764Connection::connect(
        port,
        player_name,
        player_uuid,
        chat_text_sender,
        chat_text_receiver,
    )?))
}

struct Mcp764Connection {
    run: Arc<RwLock<bool>>,
}

impl Mcp764Connection {
    fn connect(
        port: u16,
        player_name: String,
        player_uuid: Uuid,
        chat_text_sender: Sender<String>,
        chat_text_receiver: Receiver<String>,
    ) -> Result<Self> {
        let tcp_stream = TcpStream::connect(("localhost", port))?;
        let mcp = Mcp764 {
            state: Mutex::new(State::Handshaking),
            tcp_stream: Mutex::new(tcp_stream),
        };

        mcp.login(port, player_name, player_uuid)?;
        mcp.configure()?;
        mcp.play(chat_text_sender, chat_text_receiver)
    }
}

struct Mcp764 {
    state: Mutex<State>,
    tcp_stream: Mutex<TcpStream>,
}

impl Mcp764 {
    fn login(&self, port: u16, player_name: String, player_uuid: Uuid) -> Result<()> {
        self.write_packet(ServerBoundPacket::Handshake {
            protocol_version: VarInt::from(764),
            server_address: MinecraftString::try_from("localhost".to_owned())?,
            server_port: port,
            next_state: State::Login,
        })?;
        self.write_packet(ServerBoundPacket::LoginStart {
            name: MinecraftString::try_from(player_name)?,
            player_uuid,
        })?;
        if let ClientBoundPacket::LoginSuccess = self.read_packet()? {
            self.write_packet(ServerBoundPacket::LoginAcknowledged)?;
            // println!("Login success!");
        } else {
            return Err(anyhow!("Login failed."));
        }

        Ok(())
    }

    fn configure(&self) -> Result<()> {
        loop {
            match self.read_packet()? {
                ClientBoundPacket::Disconnect { reason } => return Err(anyhow!("Disconnected by server. {}", reason.into_inner())),
                ClientBoundPacket::FinishConfiguration => {
                    break;
                }
                ClientBoundPacket::KeepAlive { id } => {
                    self.write_packet(ServerBoundPacket::KeepAlive { id })?
                }
                _ => {}
            }
        }

        self.write_packet(ServerBoundPacket::ClientInformation {
            locale: MinecraftString("en_GB".to_owned()),
            view_distance: 8,
            chat_mode: VarInt::from(0),
            chat_colors: true,
            display_skin_parts: 0,
            main_hand: VarInt::from(1),
            enable_text_filtering: false,
            allow_server_listings: true,
        })?;
        self.write_packet(ServerBoundPacket::FinishConfiguration)?;

        // println!("Configuration success!");

        Ok(())
    }

    fn play(
        self,
        chat_text_sender: Sender<String>,
        chat_text_receiver: Receiver<String>,
    ) -> Result<Mcp764Connection> {
        let mcp = Arc::new(Mutex::new(self));
        let run = Arc::new(RwLock::new(true));

        {
            let mcp = mcp.clone();
            let run = run.clone();
            thread::spawn(move || {
                // println!("Reading packets...");

                while *run.read() {
                    let packet = mcp.lock().read_packet()?;
                    match packet {
                        ClientBoundPacket::Disconnect { reason } => {
                            return Err(anyhow!("Disconnected by server. {}", reason.into_inner()));
                        }
                        ClientBoundPacket::KeepAlive { id } => {
                            mcp.lock()
                                .write_packet(ServerBoundPacket::KeepAlive { id })?;
                        }
                        ClientBoundPacket::PlayerChatMessage(player_chat_message) => {
                            // let resv_message_count = i32::from(player_chat_message.previous_messages.total_previous_message) as usize;
                            
                            // resv_message_count += 1;
                            
                            // let mut acknowledged = acknowledged.lock();
                            // for i in 0..dbg!(resv_message_count).min(20) {
                            //     let mask = 1 << (i % 8);
                            //     acknowledged[i / 8] |= mask;
                            // }

                            chat_text_sender.send(player_chat_message.body.message.into_inner())?;
                        }
                        ClientBoundPacket::SystemChatMessage { content } => {
                            // resv_message_count += 1;

                            // let mut acknowledged = ack.lock();
                            // // for i in 0..dbg!(resv_message_count).min(20) {
                            // //     let mask = 1 << (i % 8);
                            // //     acknowledged[i / 8] |= mask;
                            // // }

                            // dbg!(*acknowledged);

                            // let mut acknowledged = acknowledged.lock();
                            // *acknowledged = [ 0b00000001, 0x00, 0x00 ];

                            // let mcp = mcp.lock();
                            // mcp.write_packet(dbg!(ServerBoundPacket::MessageAcknowledgment { message_count: VarInt::from(1) }))?;

                            chat_text_sender.send(content.into_inner())?;
                        }
                        ClientBoundPacket::Unknown { packet_id, .. } => {
                            if [0x1C, 0x67].contains(&packet_id) {
                                println!("{:?}", packet);
                            }
                        }
                        _ => {}
                    }
                }

                Ok(())
            });
        }

        {
            let run = run.clone();
            thread::spawn(move || {
                while *run.read() {
                    let text = chat_text_receiver.recv()?;
                    if text.starts_with("/") {
                        let mcp = mcp.lock();
                        mcp.write_packet(ServerBoundPacket::ChatCommand {
                            command: MinecraftString::try_from(text[1..].to_owned())?,
                            timestamp: Local::now().timestamp_millis(),
                            salt: rand::random(),
                            message_count: VarInt::from(0),
                            acknowledged: [0, 0, 0],
                        })?;
                    } else {
                        let mcp = mcp.lock();
                        mcp.write_packet(ServerBoundPacket::ChatMessage {
                            message: MinecraftString::try_from(text)?,
                            timestamp: Local::now().timestamp_millis(),
                            salt: rand::random(),
                            signature: None,
                            message_count: VarInt::from(0),
                            acknowledged: [0, 0, 0],
                        })?;
                    }
                }

                Ok::<(), anyhow::Error>(())
            });
        }

        Ok(Mcp764Connection { run })
    }

    fn write_packet(&self, packet: ServerBoundPacket) -> Result<()> {
        let change_state = packet.state_changer();
        let mut buffer = Vec::new();

        let mut state = self.state.lock();

        buffer.write_var_int(packet.packet_id(*state))?;
        buffer.write(&packet.payload()?)?;

        let mut tcp_stream = self.tcp_stream.lock();
        tcp_stream.write_var_int(VarInt::from(buffer.len() as i32))?;
        tcp_stream.write(&buffer)?;

        *state = change_state(*state);

        Ok(())
    }

    fn read_packet(&self) -> Result<ClientBoundPacket> {
        let mut tcp_stream = self.tcp_stream.lock();
        let length = tcp_stream.read_var_int()?;
        let mut content = vec![0; i32::from(length) as usize];
        tcp_stream.read_exact(&mut content)?;
        let mut content = Cursor::new(content);
        let packet_id = content.read_var_int()?;
        let mut payload = Vec::new();
        content.read_to_end(&mut payload)?;

        ClientBoundPacket::from(*self.state.lock(), packet_id, &payload)
    }
}

#[derive(Debug)]
enum ClientBoundPacket {
    Unknown { packet_id: i32 },
    LoginSuccess,
    Disconnect { reason: MinecraftString<262144> },
    FinishConfiguration,
    KeepAlive { id: Long },
    PlayerChatMessage(PlayerChatMessage),
    SystemChatMessage { content: MinecraftString<262144> },
}

#[derive(Debug)]
struct PlayerChatMessage {
    header: PlayerChatMessageHeader,
    body: PlayerChatMessageBody,
    previous_messages: PlayerChatPreviousMessages,
}

#[derive(Debug)]
struct PlayerChatMessageHeader {
    sender: Uuid,
    index: VarInt,
    message_signature: Option<[UByte; 256]>,
}

#[derive(Debug)]
struct PlayerChatMessageBody {
    message: MinecraftString<256>,
    timestamp: Long,
    salt: Long,
}

#[derive(Debug)]
struct PlayerChatPreviousMessages {
    total_previous_message: VarInt
}

impl ClientBoundPacket {
    fn from(state: State, packet_id: VarInt, payload: &[u8]) -> Result<ClientBoundPacket> {
        let mut payload = Cursor::new(payload);
        match (state, i32::from(packet_id)) {
            (State::Login, 0x00) | (State::Configuration, 0x01) | (State::Play, 0x1B) => {
                let reason = payload.read_minecraft_string()?;
                Ok(ClientBoundPacket::Disconnect { reason } )
            }
            (State::Login, 0x02) => Ok(ClientBoundPacket::LoginSuccess),
            (State::Configuration, 0x02) => Ok(ClientBoundPacket::FinishConfiguration),
            (State::Configuration, 0x03) | (State::Play, 0x24) => {
                let id = payload.read_long()?;
                Ok(ClientBoundPacket::KeepAlive { id })
            }
            (State::Play, 0x37) => {
                let sender = payload.read_uuid()?;
                let index = payload.read_var_int()?;
                let message_signature = if payload.read_bool()? {
                    let mut buf = [0; 256];
                    payload.read_exact(&mut buf)?;
                    Some(buf)
                } else {
                    None
                };
                let message = payload.read_minecraft_string()?;
                let timestamp = payload.read_long()?;
                let salt = payload.read_long()?;
                let total_previous_message = payload.read_var_int()?;

                Ok(ClientBoundPacket::PlayerChatMessage(PlayerChatMessage {
                    header: PlayerChatMessageHeader {
                        sender,
                        index,
                        message_signature,
                    },
                    body: PlayerChatMessageBody {
                        message,
                        timestamp,
                        salt,
                    },
                    previous_messages: PlayerChatPreviousMessages { total_previous_message }
                }))
            }
            (State::Play, 0x67) => {
                let content = payload.read_minecraft_string()?;

                Ok(ClientBoundPacket::SystemChatMessage { content })
            }
            (_, packet_id) => Ok(ClientBoundPacket::Unknown { packet_id }),
        }
    }
}

#[derive(Debug)]
enum ServerBoundPacket {
    Handshake {
        protocol_version: VarInt,
        server_address: MinecraftString<255>,
        server_port: UShort,
        next_state: State,
    },
    LoginStart {
        name: MinecraftString<16>,
        player_uuid: Uuid,
    },
    LoginAcknowledged,
    KeepAlive {
        id: Long,
    },
    FinishConfiguration,
    ClientInformation {
        locale: MinecraftString<16>,
        view_distance: Byte,
        chat_mode: VarInt,
        chat_colors: bool,
        display_skin_parts: UByte,
        main_hand: VarInt,
        enable_text_filtering: bool,
        allow_server_listings: bool,
    },
    ChatMessage {
        message: MinecraftString<256>,
        timestamp: Long,
        salt: Long,
        signature: Option<[UByte; 256]>,
        message_count: VarInt,
        acknowledged: [UByte; 3],
    },
    ChatCommand {
        command: MinecraftString<256>,
        timestamp: Long,
        salt: Long,
        message_count: VarInt,
        acknowledged: [UByte; 3],
    },
    MessageAcknowledgment {
        message_count: VarInt
    }
}

impl ServerBoundPacket {
    fn packet_id(&self, state: State) -> VarInt {
        match self {
            ServerBoundPacket::Handshake { .. } => VarInt::from(0x00),
            ServerBoundPacket::LoginStart { .. } => VarInt::from(0x00),
            ServerBoundPacket::LoginAcknowledged => VarInt::from(0x03),
            ServerBoundPacket::KeepAlive { .. } => match state {
                State::Configuration => VarInt::from(0x03),
                State::Play => VarInt::from(0x14),
                _ => unimplemented!(),
            },
            ServerBoundPacket::FinishConfiguration => VarInt::from(0x02),
            ServerBoundPacket::ClientInformation { .. } => match state {
                State::Configuration => VarInt::from(0x00),
                State::Play => VarInt::from(0x09),
                _ => unimplemented!(),
            },
            ServerBoundPacket::ChatMessage { .. } => VarInt::from(0x05),
            ServerBoundPacket::ChatCommand { .. } => VarInt::from(0x04),
            ServerBoundPacket::MessageAcknowledgment { .. } => VarInt::from(0x03),
        }
    }

    fn payload(self) -> Result<Vec<u8>> {
        let mut buffer = Vec::new();
        match self {
            ServerBoundPacket::Handshake {
                protocol_version,
                server_address,
                server_port,
                next_state,
            } => {
                buffer.write_var_int(protocol_version)?;
                buffer.write_minecraft_string(&server_address)?;
                buffer.write_ushort(server_port)?;
                buffer.write_state(next_state)?;
            }
            ServerBoundPacket::LoginStart { name, player_uuid } => {
                buffer.write_minecraft_string(&name)?;
                buffer.write_uuid(player_uuid)?;
            }
            ServerBoundPacket::LoginAcknowledged => {}
            ServerBoundPacket::KeepAlive { id } => {
                buffer.write_long(id)?;
            }
            ServerBoundPacket::FinishConfiguration => {}
            ServerBoundPacket::ClientInformation {
                locale,
                view_distance,
                chat_mode,
                chat_colors,
                display_skin_parts,
                main_hand,
                enable_text_filtering,
                allow_server_listings,
            } => {
                buffer.write_minecraft_string(&locale)?;
                buffer.write_byte(view_distance)?;
                buffer.write_var_int(chat_mode)?;
                buffer.write_bool(chat_colors)?;
                buffer.write_ubyte(display_skin_parts)?;
                buffer.write_var_int(main_hand)?;
                buffer.write_bool(enable_text_filtering)?;
                buffer.write_bool(allow_server_listings)?;
            }
            ServerBoundPacket::ChatMessage {
                message,
                timestamp,
                salt,
                signature,
                message_count,
                acknowledged,
            } => {
                buffer.write_minecraft_string(&message)?;
                buffer.write_long(timestamp)?;
                buffer.write_long(salt)?;
                match signature {
                    Some(sig) => {
                        buffer.write_bool(true)?;
                        buffer.write_all(&sig)?;
                    }
                    None => {
                        buffer.write_bool(false)?;
                    }
                }
                buffer.write_var_int(message_count)?;
                buffer.write_all(&acknowledged)?;
            }
            ServerBoundPacket::ChatCommand {
                command,
                timestamp,
                salt,
                message_count,
                acknowledged,
            } => {
                buffer.write_minecraft_string(&command)?;
                buffer.write_long(timestamp)?;
                buffer.write_long(salt)?;
                buffer.write_var_int(VarInt::from(0))?; // indicates no signed arguments
                buffer.write_var_int(message_count)?;
                buffer.write_all(&acknowledged)?;
            }
            ServerBoundPacket::MessageAcknowledgment { message_count } => {
                buffer.write_var_int(message_count)?;
            }
        }
        Ok(buffer)
    }

    fn state_changer(&self) -> Box<dyn Fn(State) -> State> {
        match self {
            ServerBoundPacket::Handshake { next_state, .. } => {
                let state = *next_state;
                Box::new(move |_| state)
            }
            ServerBoundPacket::LoginAcknowledged => Box::new(|_| State::Configuration),
            ServerBoundPacket::FinishConfiguration => Box::new(|_| State::Play),
            _ => Box::new(|state| state),
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum State {
    Handshaking,
    Login,
    Configuration,
    Play,
}

trait WriteExt {
    fn write_byte(&mut self, value: i8) -> Result<()>;
    fn write_ubyte(&mut self, value: u8) -> Result<()>;
    fn write_bool(&mut self, value: bool) -> Result<()>;
    fn write_ushort(&mut self, value: UShort) -> Result<()>;
    fn write_long(&mut self, value: Long) -> Result<()>;
    fn write_minecraft_string<const MAX_LENGTH: usize>(
        &mut self,
        value: &MinecraftString<MAX_LENGTH>,
    ) -> Result<()>;
    fn write_state(&mut self, value: State) -> Result<()>;
    fn write_uuid(&mut self, value: Uuid) -> Result<()>;
}

impl<W: Write> WriteExt for W {
    fn write_byte(&mut self, value: i8) -> Result<()> {
        self.write(&value.to_be_bytes())?;
        Ok(())
    }

    fn write_ubyte(&mut self, value: u8) -> Result<()> {
        self.write(&value.to_be_bytes())?;
        Ok(())
    }

    fn write_bool(&mut self, value: bool) -> Result<()> {
        self.write_ubyte(if value { 0x01 } else { 0x00 })
    }

    fn write_ushort(&mut self, value: UShort) -> Result<()> {
        self.write(&value.to_be_bytes())?;
        Ok(())
    }

    fn write_long(&mut self, value: Long) -> Result<()> {
        self.write(&value.to_be_bytes())?;
        Ok(())
    }

    fn write_minecraft_string<const MAX_LENGTH: usize>(
        &mut self,
        value: &MinecraftString<MAX_LENGTH>,
    ) -> Result<()> {
        self.write_var_int(VarInt::from(value.0.len() as i32))?;
        self.write(value.0.as_bytes())?;
        Ok(())
    }

    fn write_state(&mut self, value: State) -> Result<()> {
        self.write_var_int(match value {
            State::Handshaking => VarInt::from(0),
            State::Login => VarInt::from(2),
            State::Configuration => VarInt::from(3),
            State::Play => VarInt::from(4),
        })?;
        Ok(())
    }

    fn write_uuid(&mut self, value: Uuid) -> Result<()> {
        self.write(&value.as_u128().to_be_bytes())?;
        Ok(())
    }
}

trait ReadExt {
    fn read_ubyte(&mut self) -> Result<u8>;
    fn read_bool(&mut self) -> Result<bool>;
    fn read_long(&mut self) -> Result<Long>;
    fn read_uuid(&mut self) -> Result<Uuid>;
    fn read_minecraft_string<const MAX_LENGTH: usize>(
        &mut self,
    ) -> Result<MinecraftString<MAX_LENGTH>>;
}

impl<R: Read> ReadExt for R {
    fn read_ubyte(&mut self) -> Result<u8> {
        let mut buf = [0; 1];
        self.read_exact(&mut buf)?;
        Ok(UByte::from_be_bytes(buf))
    }

    fn read_bool(&mut self) -> Result<bool> {
        if self.read_ubyte()? == 0x00 {
            Ok(false)
        } else {
            Ok(true)
        }
    }

    fn read_long(&mut self) -> Result<Long> {
        let mut buf = [0; 8];
        self.read_exact(&mut buf)?;
        Ok(Long::from_be_bytes(buf))
    }

    fn read_uuid(&mut self) -> Result<Uuid> {
        let mut buf = [0; 16];
        self.read_exact(&mut buf)?;
        Ok(Uuid::from_u128(u128::from_be_bytes(buf)))
    }

    fn read_minecraft_string<const MAX_LENGTH: usize>(
        &mut self,
    ) -> Result<MinecraftString<MAX_LENGTH>> {
        let len = i32::from(self.read_var_int()?) as usize;
        let mut buf = vec![0; len];
        self.read_exact(&mut buf)?;
        MinecraftString::try_from(String::from_utf8(buf)?)
    }
}

#[derive(Debug)]
struct MinecraftString<const MAX_LENGTH: usize>(String);

impl<const MAX_LENGTH: usize> MinecraftString<MAX_LENGTH> {
    fn into_inner(self) -> String {
        self.0
    }
}

impl<const MAX_LENGTH: usize> TryFrom<String> for MinecraftString<MAX_LENGTH> {
    fn try_from(inner: String) -> Result<Self> {
        if inner.len() >= MAX_LENGTH {
            return Err(anyhow!(
                "Max length of MinecraftString<{MAX_LENGTH}> exceeded! ({} > {MAX_LENGTH})",
                inner.len()
            ));
        }

        Ok(MinecraftString::<MAX_LENGTH>(inner.into()))
    }

    type Error = anyhow::Error;
}

type UShort = u16;
type Long = i64;
type Byte = i8;
type UByte = u8;
