use std::future::Future;
use std::io::{self, Read};
use std::str;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use futures::FutureExt;
use futures::channel::oneshot;
use futures::future::{self, BoxFuture};
use log::{trace, debug, info, warn};
use serialport::{SerialPort, ClearBuffer};
use thiserror::Error;
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};

const RESPONSE_WAIT_PERIOD: Duration = Duration::from_millis(200);

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

  #[error("error communicating via serial port: {}", source)]
  SerialIOError {
    #[from]
    source: std::io::Error
  },

  #[error("projector did not send expected preamble response")]
  CommandSendInvalidState,

  #[error("response contained invalid data: source")]
  ResponseInvalidString {
    #[from]
    source: str::Utf8Error
  },

  #[error("response did not match expected format: {:?}", 0)]
  ResponseUnexpectedFormat(String),

  #[error("projector returned an error ('Block item')")]
  ResponseBlockItem
}

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Clone)]
pub enum Command {
  /// A special pseudo-command to end the processing thread
  Stop,

  /// A getter command that has no side effects but expects a response
  Get(String),

  /// A setter command changes the projector's state
  Set((String, String)),

  /// A special command to sleep the processing thread.
  ///
  /// This is intended to work around potential serial interface crashes when
  /// sending commands while the projector is transitioning between power
  /// states. Clients can send this sleep command to temporarily block the
  /// processing thread if they notice (via their own `pow=?` commands) that the
  /// projector has transitioned states via external means (i.e. user pressing
  /// the power button).
  ///
  /// Note that the processing thread already includes a safety wait when state
  /// transitions are requested via this library.
  Sleep(Duration),
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

impl From<(&str, String)> for Command {
  fn from(tup: (&str, String)) -> Self {
    Command::Set((tup.0.to_string(), tup.1))
  }
}

pub type CommandResult = Result<Option<String>>;

#[derive(Debug)]
struct SubmittedCommand {
  command: Command,
  tx: oneshot::Sender<CommandResult>
}

pub struct ProjectorControl {
  cmd_tx: UnboundedSender<SubmittedCommand>,
}

impl ProjectorControl {
  pub fn new(port: Box<dyn SerialPort>) -> ProjectorControl {
    let (cmd_tx, cmd_rx) = unbounded_channel();
    spawn_command_thread(port, cmd_rx);

    ProjectorControl { cmd_tx }
  }

  /// Submits a command for future processing.
  ///
  /// The response, if any, will be available by `.await`-ing on the returned
  /// future. The actual command execution takes place on a background thread
  /// upon which commands are executed in the order they are received (at
  /// roughly 100ms intervals).
  ///
  /// Note that this function does have immediate side-effects as the command
  /// will be queued immediately rather than when `.await` is called on the
  /// returned future.
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

fn read_response(port: &mut Box<dyn SerialPort>, command: &str) -> Result<Option<String>> {
  let mut response: Vec<u8> = Vec::with_capacity(64);
  let mut buf: Vec<u8> = vec![0; 32];

  let instant = Instant::now();
  while instant.elapsed() < RESPONSE_WAIT_PERIOD {
    match port.read(buf.as_mut_slice()) {
      Ok(n) => response.extend_from_slice(&buf[..n]),

      // keep trying until the time has elapsed
      Err(ref e) if e.kind() == io::ErrorKind::TimedOut => (),

      // bubble up all other errors
      Err(e) => return Err(Error::SerialIOError { source: e })
    }
  }

  let response = str::from_utf8(&response)?;
  trace!("full response: {:?}", response);

  // the device seems to echo characters, so expect the first line to be what we
  // just sent and strip it off
  if !response.starts_with(command) {
    return Err(Error::ResponseUnexpectedFormat(response.to_string()));
  }

  let response = &response[command.len()..].trim();
  if response.is_empty() {
    Ok(None)
  } else if response.starts_with('*') && response.ends_with('#') {
    let truncated = &response[1..response.len() - 1];
    if truncated.to_ascii_lowercase() == "block item" {
      Err(Error::ResponseBlockItem)
    } else {
      Ok(Some(truncated.to_string()))
    }
  } else {
    Err(Error::ResponseUnexpectedFormat(response.to_string()))
  }
}

fn send_get(port: &mut Box<dyn SerialPort>, key: &str) -> CommandResult {
  port.clear(ClearBuffer::All)?;
  port.write_all(b"\r")?;

  let mut buf: [u8; 1] = [0; 1];
  port.read_exact(&mut buf)?;
  trace!("send_get: prompt buf: {:?}", str::from_utf8(&buf));

  if buf[0] as char != '>' {
    return Err(Error::CommandSendInvalidState);
  }

  let command = format!("*{}=?#\r", key);
  port.write_all(command.as_bytes())?;
  trace!("send_get: wrote query: {:?}", command);

  read_response(port, &command)
}

fn send_set(port: &mut Box<dyn SerialPort>, key: &str, value: &str) -> CommandResult {
  port.clear(ClearBuffer::Input)?;

  port.write_all(b"\r")?;

  let mut buf: [u8; 1] = [0; 1];
  port.read_exact(&mut buf)?;
  trace!("send_set: prompt buf: {:?}", str::from_utf8(&buf));
  if buf[0] != b'>' {
    return Err(Error::CommandSendInvalidState);
  }

  let command = format!("*{}={}#\r", key, value);
  port.write_all(command.as_bytes())?;
  trace!("send_set: wrote command: {:?}", command);

  read_response(port, &command)
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
        Command::Sleep(d) => {
          thread::sleep(*d);
          Ok(None)
        }
      };

      debug!("command {:?} result: {:?}", &cmd.command, &result);

      if let Err(e) = cmd.tx.send(result) {
        // we can't do much if this fails, but dropping it normally after this
        // iteration will at least raise Cancelled on the other end (though the
        // other end probably no longer exists)
        debug!("command ({:?}) response send failed: {:?}", &cmd.command, e);
      }

      if let Command::Stop = &cmd.command {
        break;
      }

      // wait a bit between commands for safety
      let delay_millis = match cmd.command {
        Command::Set((k, v)) => {
          if k.to_ascii_lowercase() == "pow" {
            if v.to_ascii_lowercase() == "off" {
              // power off takes longer
              60_000
            } else {
              30_000
            }
          } else {
            500
          }
        },
        _ => 1
      };

      // hack: sending commands too quickly after powering on crashes the serial
      // interface, so block the processing thread for a bit
      // note that this does nothing to protect us if we accidentally send commands
      // after the user presses buttons on the projector - we'll need to rely on
      trace!("waiting {}ms after command", delay_millis);
      thread::sleep(Duration::from_millis(delay_millis));
    }
  })
}
