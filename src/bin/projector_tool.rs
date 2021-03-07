
use std::fmt;
use std::time::Duration;

use benq_control::{ProjectorControl, Command};
use color_eyre::eyre::{Result, Error, Context, eyre};
use log::*;
use structopt::StructOpt;

fn parse_command(s: &str) -> Result<Command> {
  let tokens = s.splitn(2, '=').collect::<Vec<_>>();
  match &tokens[..] {
    // assume no '=' means get, since =? triggers shell expansion and requires
    // single quoting
    [lhs] | [lhs, "?"] => Ok(Command::Get(lhs.to_ascii_lowercase())),
    [lhs, rhs] => Ok(Command::Set((lhs.to_ascii_lowercase(), rhs.to_ascii_lowercase()))),
    _ => Err(eyre!("invalid command string: {}", s))
  }
}

fn parse_volume(s: &str) -> Result<u8> {
  // TODO: is this 1-100?
  let v = s.parse::<u8>().with_context(|| format!("invalid volume: {}", s))?;

  if v > 100 {
    return Err(eyre!("volume must be 0-100"));
  }

  Ok(v)
}

#[derive(Debug, Clone, StructOpt)]
#[structopt(rename_all = "kebab-case")]
enum PowerAction {
  On,
  Off,
  Status
}

#[derive(Debug, Clone, StructOpt)]
#[structopt(rename_all = "kebab-case")]
enum SourceAction {
  #[structopt(aliases = &["hdmi1"])]
  HDMI,
  HDMI2,
  RGB,
  Status
}

impl fmt::Display for SourceAction {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    write!(f, "{}", match self {
      SourceAction::HDMI => "hdmi",
      SourceAction::HDMI2 => "hdmi2",
      SourceAction::RGB => "rgb",
      SourceAction::Status => "status"
  })
  }
}

#[derive(Debug, Clone, StructOpt)]
#[structopt(rename_all = "kebab-case")]
enum VolumeAction {
  Up,
  Down,
  Set {
    #[structopt(parse(try_from_str = parse_volume))]
    value: u8
  },
  Status
}

#[derive(Debug, Clone, StructOpt)]
#[structopt(rename_all = "kebab-case")]
enum MuteAction {
  On,
  Off,
  Status
}

#[derive(Debug, Clone, StructOpt)]
struct ExecAction {
  #[structopt(parse(try_from_str = parse_command))]
  command: Command
}

#[derive(Debug, Clone, StructOpt)]
#[structopt(rename_all = "kebab-case")]
enum Action {
  /// Sets or queries projector power state
  #[structopt(aliases = &["pow", "p"])]
  Power(PowerAction),

  /// Sets or queries the projector's current input. Returns an error if the
  /// projector is not currently powered on.
  #[structopt(aliases = &["sour", "s", "input", "i"])]
  Source(SourceAction),

  /// Sets or queries the projector's current volume. Returns an error if the
  /// projector is not currently powered on.
  #[structopt(aliases = &["vol", "v"])]
  Volume(VolumeAction),

  /// Sets or queries the projector's mute state. Returns an error if the
  /// projector is not currently powered on.
  #[structopt(aliases = &["m"])]
  Mute(MuteAction),

  /// Executes an arbitrary command. Refer to the documentation for a full list
  /// of commands.
  ///
  /// Note that this command accepts slightly different syntax: `key=value` to
  /// set an option and `key=?` or just `key` to query a value.
  #[structopt(aliases = &["e"])]
  Exec(ExecAction),
}

#[derive(Debug, Clone, StructOpt)]
#[structopt(name = "projector-tool")]
struct Options {
  /// projector serial port device path
  #[structopt(
    long, short,
    default_value = "/dev/ttyUSB0",
    global = true,
    env = "PROJECTOR_DEVICE"
  )]
  device: String,

  #[structopt(
    long, short,
    default_value = "115200",
    global = true,
    env = "PROJECTOR_BAUD_RATE"
  )]
  baud_rate: u32,

  #[structopt(subcommand)]
  action: Action
}

async fn handle_power(
  _opts: &Options,
  action: &PowerAction,
  controller: ProjectorControl
) -> Result<()> {
  let res = match action {
    PowerAction::On => controller.submit_command(("pow", "on")),
    PowerAction::Off => controller.submit_command(("pow", "off")),
    PowerAction::Status => controller.submit_command("pow")
  }.await?;

  debug!("power response: {:?}", res);
  if let Some(r) = res {
    println!("{}", r);
  }

  Ok(())
}

async fn handle_source(
  _opts: &Options,
  action: &SourceAction,
  controller: ProjectorControl
) -> Result<()> {
  let res = match action {
    SourceAction::Status => controller.submit_command("sour"),
    source => controller.submit_command(("sour", source.to_string()))
  }.await?;

  debug!("source response: {:?}", res);
  if let Some(r) = res {
    println!("{}", r);
  }

  Ok(())
}

async fn handle_volume(
  _opts: &Options,
  action: &VolumeAction,
  controller: ProjectorControl
) -> Result<()> {
  let res = match action {
    VolumeAction::Status => controller.submit_command("vol"),
    VolumeAction::Up => controller.submit_command(("vol", "+")),
    VolumeAction::Down => controller.submit_command(("vol", "-")),
    VolumeAction::Set { value } => controller.submit_command(("vol", value.to_string())),
  }.await?;

  debug!("volume response: {:?}", res);
  if let Some(r) = res {
    println!("{}", r);
  }

  Ok(())
}

async fn handle_mute(
  _opts: &Options,
  action: &MuteAction,
  controller: ProjectorControl
) -> Result<()> {
  let res = match action {
    MuteAction::Status => controller.submit_command("mute"),
    MuteAction::On => controller.submit_command(("mute", "on")),
    MuteAction::Off => controller.submit_command(("mute", "off")),
  }.await?;

  debug!("mute response: {:?}", res);
  if let Some(r) = res {
    println!("{}", r);
  }

  Ok(())
}

async fn handle_exec(
  _opts: &Options,
  action: &ExecAction,
  controller: ProjectorControl
) -> Result<()> {
  info!("exec command: {:?}", action.command);

  let res = controller.submit_command(action.command.clone()).await?;
  debug!("exec response: {:?}", res);

  if let Some(r) = res {
    println!("{}", r);
  }

  Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
  color_eyre::install()?;

  let env = env_logger::Env::default()
    .filter_or("PROJECTOR_LOG", "info")
    .write_style_or("PROJECTOR_STYLE", "always");

  env_logger::Builder::from_env(env)
    .target(env_logger::Target::Stderr)
    .init();

  let opts: Options = Options::from_args();
  debug!("options: {:?}", opts);

  let port = serialport::new(&opts.device, opts.baud_rate)
    .timeout(Duration::from_millis(50))
    .open()?;

  let controller = ProjectorControl::new(port);

  match &opts.action {
    Action::Power(action) => handle_power(&opts, action, controller).await?,
    Action::Source(action) => handle_source(&opts, action, controller).await?,
    Action::Volume(action) => handle_volume(&opts, action, controller).await?,
    Action::Mute(action) => handle_mute(&opts, action, controller).await?,
    Action::Exec(action) => handle_exec(&opts, action, controller).await?,
  };

  Ok(())
}
