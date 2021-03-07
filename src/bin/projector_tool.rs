use std::path::PathBuf;

use benq_control::{ProjectorControl, Command};
use color_eyre::eyre::{Result, Error, Context, eyre};
use log::*;
use structopt::StructOpt;

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
  HDMI,
  HDMI2,
  RGB,
  Status
}
#[derive(Debug, Clone, StructOpt)]
struct ExecAction {
  // TODO
}

#[derive(Debug, Clone, StructOpt)]
#[structopt(rename_all = "kebab-case")]
enum Action {
  /// Sets or queries projector power state
  Power(PowerAction),

  /// Sets or queries the projector's current input
  Source(SourceAction),

  /// Executes an arbitrary command
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
  opts: &Options,
  action: &PowerAction,
  controller: ProjectorControl
) -> Result<()> {
  let res = match action {
    PowerAction::On => controller.submit_command(("pow", "on")),
    PowerAction::Off => controller.submit_command(("pow", "off")),
    PowerAction::Status => controller.submit_command("pow")
  }.await;

  info!("power response: {:?}", res);

  Ok(())
}

async fn handle_source(
  opts: &Options,
  action: &SourceAction,
  controller: ProjectorControl
) -> Result<()> {
  Ok(())
}

async fn handle_exec(
  opts: &Options,
  action: &ExecAction,
  controller: ProjectorControl
) -> Result<()> {
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

  let port = serialport::new(&opts.device, opts.baud_rate).open()?;

  let controller = ProjectorControl::new(port);

  match &opts.action {
    Action::Power(action) => handle_power(&opts, action, controller).await?,
    Action::Source(action) => handle_source(&opts, action, controller).await?,
    Action::Exec(action) => handle_exec(&opts, action, controller).await?
  };

  Ok(())
}
