use std::thread::{self, JoinHandle};
use std::future::Future;

use futures::FutureExt;
use futures::channel::oneshot;
use futures::future::{self, BoxFuture};
use log::{trace, debug, info, warn};
use serialport::{SerialPort, ClearBuffer};
use thiserror::Error;
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};

#[derive(Error, Debug)]
pub enum Error {
  #[error("command was cancelled")]
  Cancelled {
    command: Command
  },

  #[error("command could not be sent: {:?}", command)]
  CommandSendError {
    /// The command that could not be submitted
    command: Command,
  },

  #[error("serial port error: {}", source)]
  SerialError {
    #[from]
    source: serialport::Error
  },

  #[error("error writing command to serial port: {}", source)]
  SerialWriteError {
    #[from]
    source: std::io::Error
  }
}

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Clone)]
pub enum Command {
  /// A special pseudo-command to end the processing thread
  Stop,

  /// A getter command that has no side effects but expects a response
  Get(String),

  /// A setter command changes the projector's state
  Set((String, String))
}

impl From<&str> for Command {
  fn from(s: &str) -> Self {
    Command::Get(s.to_string())
  }
}

impl From<(&str, &str)> for Command {
  fn from(tup: (&str, &str)) -> Self {
    Command::Set((tup.0.to_string(), tup.1.to_string()))
  }
}

// pub enum CommandResult {
//   Pending,
//   Err(Error),
//   OkGet,
//   OkSet(String)
//}

pub type CommandResult = Result<Option<String>>;

#[derive(Debug)]
struct SubmittedCommand {
  command: Command,
  tx: oneshot::Sender<CommandResult>
}

pub struct ProjectorControl {
  cmd_tx: UnboundedSender<SubmittedCommand>,
  cmd_join: JoinHandle<()>,
}

impl ProjectorControl {
  pub fn new(port: Box<dyn SerialPort>) -> ProjectorControl {
    let (cmd_tx, cmd_rx) = unbounded_channel();
    let cmd_join = spawn_command_thread(port, cmd_rx);

    ProjectorControl { cmd_tx, cmd_join }
  }

  pub fn submit_command(&self, command: impl Into<Command>) -> BoxFuture<CommandResult> {
    let command = command.into();
    let (tx, rx) = oneshot::channel::<CommandResult>();
    let message = SubmittedCommand {
      command: command.clone(),
      tx
    };

    match self.cmd_tx.send(message) {
      // flatten the oneshot's Cancelled case
      Ok(()) => rx.map(|r| match r {
        Ok(v) => v,
        Err(_) => Err(Error::Cancelled {
          command
        })
      }).boxed(),

      Err(_e) => future::ready(Err(Error::CommandSendError {
        command
      })).boxed()
    }
  }

  /// Stop the processing thread.
  ///
  /// This consumes the ProjectorControl instance as it will stop all further
  /// command processing and close the serial port.
  pub fn stop(self) -> impl Future<Output = CommandResult> {
    // annoyingly we basically have to reimplement this function due to lifetime
    // issues if we call self.submit_command() as it is fn(&self)
    let (tx, rx) = oneshot::channel::<CommandResult>();
    let message = SubmittedCommand {
      command: Command::Stop,
      tx
    };

    match self.cmd_tx.send(message) {
      // flatten the oneshot's Cancelled case
      Ok(()) => rx.map(|r| match r {
        Ok(v) => v,
        Err(_) => Err(Error::Cancelled {
          command: Command::Stop
        })
      }).boxed(),

      Err(_e) => future::ready(Err(Error::CommandSendError {
        command: Command::Stop
      })).boxed()
    }
  }
}

fn send_get(port: &mut Box<dyn SerialPort>, key: &str) -> CommandResult {
  port.clear(ClearBuffer::Input)?;  

  let cmd = format!("\r*{}=?#\r", key);
  trace!("send_get: {:?}", cmd);
  write!(port, "{}", cmd)?;

  // TODO: need to read and parse the response

  thread::sleep(std::time::Duration::from_millis(3000));


  Ok(Some("on".into()))
}

fn send_set(port: &mut Box<dyn SerialPort>, key: &str, value: &str) -> CommandResult {
  let cmd = format!("\r*{}={}#\r", key, value);
  trace!("send_get: {:?}", cmd);
  write!(port, "{}", cmd)?;
  thread::sleep(std::time::Duration::from_millis(3000));

  Ok(None)
}

fn spawn_command_thread(
  mut port: Box<dyn SerialPort>,
  mut rx: UnboundedReceiver<SubmittedCommand>
) -> JoinHandle<()> {
  thread::spawn(move || {
    while let Some(cmd) = rx.blocking_recv() {
      info!("command: {:?}", &cmd.command);

      let result = match &cmd.command {
        Command::Get(key) => send_get(&mut port, key),
        Command::Set((key, value)) => send_set(&mut port, key, value),
        Command::Stop => Ok(None),
      };

      debug!("command {:?} result: {:?}", &cmd.command, &result);

      if let Err(e) = cmd.tx.send(result) {
        // we can't do much if this fails, but dropping it normally after this
        // iteration will at least raise Cancelled on the other end
        warn!("command ({:?}) send failed: {:?}", &cmd.command, e);
      }

      if let Command::Stop = &cmd.command {
        break;
      }
    }
  })
}
