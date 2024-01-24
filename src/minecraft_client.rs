mod mcp;

use anyhow::Result;
use mcp::McpConnection;
use std::collections::VecDeque;
use std::io::{self, Read, Write};
use std::mem;
use std::sync::mpsc::{channel, Receiver, Sender, TryRecvError};
use std::sync::Arc;
use uuid::Uuid;

use crate::minecraft_server::RunningMinecraftServer;

pub struct MinecraftClient {
    name: String,
    uuid: Uuid,
}

impl MinecraftClient {
    pub fn new(name: impl Into<String>, uuid: Uuid) -> Self {
        MinecraftClient { name: name.into(), uuid }
    }

    pub fn connect_to(&self, server: &RunningMinecraftServer) -> Result<Connection> {
        let port = server.port();
        let (server_chat_text_sender, client_chat_text_receiver) = channel();
        let (client_chat_text_sender, server_chat_text_receiver) = channel();

        let mcp_connection = mcp::connect(
            port,
            self.name.clone(),
            self.uuid,
            server_chat_text_sender,
            server_chat_text_receiver,
        )?;

        Ok(Connection::new(
            client_chat_text_sender,
            client_chat_text_receiver,
            mcp_connection,
        ))
    }
}

pub struct Connection {
    chat_text_sender: Sender<String>,
    chat_text_receiver: Receiver<String>,
    write_buffer: String,
    _mcp_connection: Box<dyn McpConnection>,
}

pub struct ConnectionWriteHalf {
    chat_text_sender: Sender<String>,
    write_buffer: String,
    _mcp_connection: Arc<dyn McpConnection>,
}

impl Write for ConnectionWriteHalf {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.write_buffer.push_str(
            std::str::from_utf8(buf).map_err(|_| io::Error::from(io::ErrorKind::InvalidData))?,
        );
        let mut chats: VecDeque<String> = self
            .write_buffer
            .split('\n')
            .map(|s| s.to_owned())
            .collect();

        if let Some(leftover) = chats.pop_back() {
            self.write_buffer = leftover;
        }

        for chat in chats {
            self.chat_text_sender
                .send(chat)
                .map_err(|_| io::Error::from(io::ErrorKind::ConnectionReset))?;
        }

        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        if self.write_buffer.is_empty() {
            return Ok(());
        }

        let leftover = mem::replace(&mut self.write_buffer, String::new());
        self.chat_text_sender
            .send(leftover)
            .map_err(|_| io::ErrorKind::ConnectionReset.into())
    }
}

pub struct ConnectionReadHalf {
    chat_text_receiver: Receiver<String>,
    _mcp_connection: Arc<dyn McpConnection>,
}

impl ConnectionReadHalf {
    fn receive(&self) -> io::Result<String> {
        self.chat_text_receiver
            .recv()
            .map_err(|_| io::ErrorKind::ConnectionReset.into())
    }

    fn try_receive(&self) -> io::Result<Option<String>> {
        match self.chat_text_receiver.try_recv() {
            Ok(msg) => Ok(Some(msg)),
            Err(TryRecvError::Disconnected) => Err(io::ErrorKind::ConnectionReset.into()),
            Err(TryRecvError::Empty) => Ok(None),
        }
    }
}

impl Read for ConnectionReadHalf {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let mut buf = io::Cursor::new(buf);

        let msg = self.receive()?;
        writeln!(buf, "{msg}")?;

        while let Some(msg) = self.try_receive()? {
            writeln!(buf, "{msg}")?;
        }

        Ok(buf.position() as usize)
    }
}

impl Connection {
    pub fn split(self) -> (ConnectionReadHalf, ConnectionWriteHalf) {
        let mcp_connection: Arc<dyn McpConnection> = Arc::from(self._mcp_connection);
        let read_half = ConnectionReadHalf {
            chat_text_receiver: self.chat_text_receiver,
            _mcp_connection: mcp_connection.clone(),
        };
        let write_half = ConnectionWriteHalf {
            chat_text_sender: self.chat_text_sender,
            write_buffer: self.write_buffer,
            _mcp_connection: mcp_connection,
        };

        (read_half, write_half)
    }

    fn new(
        chat_text_sender: Sender<String>,
        chat_text_receiver: Receiver<String>,
        mcp_connection: Box<dyn McpConnection>,
    ) -> Self {
        Connection {
            chat_text_sender,
            chat_text_receiver,
            write_buffer: String::new(),
            _mcp_connection: mcp_connection,
        }
    }

    fn receive(&self) -> io::Result<String> {
        self.chat_text_receiver
            .recv()
            .map_err(|_| io::ErrorKind::ConnectionReset.into())
    }

    fn try_receive(&self) -> io::Result<Option<String>> {
        match self.chat_text_receiver.try_recv() {
            Ok(msg) => Ok(Some(msg)),
            Err(TryRecvError::Disconnected) => Err(io::ErrorKind::ConnectionReset.into()),
            Err(TryRecvError::Empty) => Ok(None),
        }
    }
}

impl Read for Connection {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let mut buf = io::Cursor::new(buf);

        let msg = self.receive()?;
        writeln!(buf, "{msg}")?;

        while let Some(msg) = self.try_receive()? {
            writeln!(buf, "{msg}")?;
        }

        Ok(buf.position() as usize)
    }
}

impl Write for Connection {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.write_buffer.push_str(
            std::str::from_utf8(buf).map_err(|_| io::Error::from(io::ErrorKind::InvalidData))?,
        );
        let mut chats: VecDeque<String> = self
            .write_buffer
            .split('\n')
            .map(|s| s.to_owned())
            .collect();

        let mut count = 0;

        if let Some(leftover) = chats.pop_back() {
            count += leftover.len();
            self.write_buffer = leftover;
        }

        for chat in chats {
            count += chat.len() + 1;
            self.chat_text_sender
                .send(chat)
                .map_err(|_| io::Error::from(io::ErrorKind::ConnectionReset))?;
        }

        Ok(count)
    }

    fn flush(&mut self) -> io::Result<()> {
        if self.write_buffer.is_empty() {
            return Ok(());
        }

        let leftover = mem::replace(&mut self.write_buffer, String::new());
        self.chat_text_sender
            .send(leftover)
            .map_err(|_| io::ErrorKind::ConnectionReset.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader};

    struct McpDummy;
    impl McpConnection for McpDummy {}

    #[test]
    fn connection_read() -> Result<()> {
        let (sender, receiver) = channel();
        let mut con = BufReader::new(Connection::new(channel().0, receiver, Box::new(McpDummy)));

        let chat = "Hello, world".to_string();

        sender.send(chat.clone())?;
        let mut buf = String::new();
        con.read_line(&mut buf)?;
        assert_eq!(format!("{chat}\n"), buf);

        Ok(())
    }

    #[test]
    fn connection_write() -> Result<()> {
        let (sender, receiver) = channel();
        let mut con = Connection::new(sender, channel().1, Box::new(McpDummy));

        writeln!(con, "Hello, world!")?;
        assert_eq!("Hello, world!", &receiver.recv()?);

        writeln!(con, "Hello wrld\nworld*")?;
        assert_eq!("Hello wrld", &receiver.recv()?);
        assert_eq!("world*", &receiver.recv()?);

        Ok(())
    }

    #[test]
    fn connection_flush() -> Result<()> {
        let (sender, receiver) = channel();
        let mut con = Connection::new(sender, channel().1, Box::new(McpDummy));

        write!(con, "Hello")?;
        assert_eq!(Err(TryRecvError::Empty), receiver.try_recv());
        con.flush()?;
        assert_eq!("Hello", receiver.recv()?);

        write!(con, "Hello...\nworld")?;
        assert_eq!("Hello...", receiver.recv()?);
        assert_eq!(Err(TryRecvError::Empty), receiver.try_recv());
        con.flush()?;
        assert_eq!("world", receiver.recv()?);

        con.flush()?;
        assert_eq!(Err(TryRecvError::Empty), receiver.try_recv());

        Ok(())
    }
}
