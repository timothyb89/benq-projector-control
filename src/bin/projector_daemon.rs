use std::sync::Arc;
use std::time::Duration;

use benq_control::{Command, ProjectorControl};
use color_eyre::eyre::{Result, Context, ContextCompat};
use futures::try_join;
use log::*;
//use simple_prometheus_exporter::{Exporter, export};
use structopt::StructOpt;
use tokio::{task, sync::RwLock};
use tide::{Body, Request, Response};
use tide::prelude::*;

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

  // /// HTTP server port
  //#[structopt(long, short, default_value = "8084", env = "PROJECTOR_PORT")]
  //port: u16,

  /// port and protocol to listen on
  #[structopt(long, short, default_value = "http://0.0.0.0:8084", env = "PROJECTOR_LISTEN")]
  listen: String
}

#[derive(Debug, Serialize)]
#[serde(tag = "power")]
enum ProjectorState {
  On {
    source: String,
    volume: u8,
    muted: bool
  },
  Off,
  Invalid
}

impl ProjectorState {
  /// Returns `true` if the projector_state is [`On`].
  fn is_on(&self) -> bool {
    matches!(self, Self::On { .. })
  }
}

type WrappedProjectorState = Arc<RwLock<ProjectorState>>;

async fn fetch_state(
  controller: &ProjectorControl,
  prev_power_state: bool
) -> Result<ProjectorState> {
  let power = controller.submit_command("pow").await?;
  if let Some("POW=ON") = power.as_deref() {
    if !prev_power_state {
      // looks like the projector turned off, send a sleep command to block
      // processing for a bit
      // note: this may have been us and will add on to the existing safety
      // sleep, oh well
      controller.submit_command(Command::Sleep(Duration::from_secs(30)));
    }

    let (source, volume, mute) = try_join!(
      controller.submit_command("sour"),
      controller.submit_command("vol"),
      controller.submit_command("mute"),
    )?;

    let source = source
      .context("source query returned empty response")?
      .trim_start_matches("SOUR=")
      .to_string();

    let volume = volume
      .context("volume query returned empty response")?
      .trim_start_matches("VOL=")
      .parse::<u8>()
      .with_context(|| format!("invalid volume"))?;

    let mute = mute
      .context("mute query returned empty response")?
      .trim_start_matches("MUTE=")
      .to_string();

    let muted = mute == "ON";

    debug!("update_state: source={:?}, volume={:?}, mute={:?}", source, volume, mute);

    Ok(ProjectorState::On {
      source,
      volume,
      muted
    })
  } else {
    if prev_power_state {
      // looks like the projector turned off, send a sleep command to block
      // processing for a bit
      // note: this may have been us and will add on to the existing safety
      // sleep, oh well
      // note: the projector takes longer to power off, so sleep 60s rather than
      // 30
      controller.submit_command(Command::Sleep(Duration::from_secs(60)));
    }

    Ok(ProjectorState::Off)
  }
}

async fn update_state(controller: &ProjectorControl, state: &WrappedProjectorState) -> Result<()> {
  let prev_power_state = {
    state.read().await.is_on()
  };

  let new_state = match fetch_state(controller, prev_power_state).await {
    Ok(state) => state,
    Err(e) => {
      warn!("state refresh failed, sleeping 60s to prevent interface crash");
      controller.submit_command(Command::Sleep(Duration::from_secs(60)));
      return Err(e).context("fetching updated projector state");
    }
  };

  let mut w = state.write().await;
  *w = new_state;

  Ok(())
}

async fn update_state_task(controller: &ProjectorControl, state: WrappedProjectorState) {
  let mut interval = tokio::time::interval(Duration::from_secs(60));
  loop {
    interval.tick().await;

    if let Err(e) = update_state(controller, &state).await {
      warn!("state update failed: {:?}", e);
    }
  }
}

#[derive(Clone)]
struct State {
  projector_state: WrappedProjectorState,
  controller: Arc<ProjectorControl>
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

  let opts = Options::from_args();

  let serial_port = serialport::new(&opts.device, opts.baud_rate)
    .timeout(Duration::from_millis(50))
    .open()?;
  let controller = Arc::new(ProjectorControl::new(serial_port));
  let projector_state = Arc::new(RwLock::new(ProjectorState::Invalid));

  // spawn a task to continuously refresh the projector's status
  let refresh_controller = Arc::clone(&controller);
  let refresh_state = Arc::clone(&projector_state);
  task::spawn(async move {
    update_state_task(&refresh_controller, refresh_state).await;
  });

  let state = State {
    projector_state: Arc::clone(&projector_state),
    controller: Arc::clone(&controller),
  };

  let mut app = tide::with_state(state);
  app.at("/status").get(|req: Request<State>| async move {
    let projector_state = req.state().projector_state.read().await;

    Ok(Body::from_json(&*projector_state)?)
  });

  app.at("/power").get(|req: Request<State>| async move {
    let controller = &req.state().controller;

    let (code, response) = match controller.submit_command("pow").await {
      Ok(Some(response)) => (200, json!({"response": response})),
      Ok(None) => (200, json!({"response": null})),
      Err(e) => (500, json!({"error": format!("{}", e)}))
    };

    Ok(
      Response::builder(code)
        .body(response)
        .build()
    )
  });

  app.at("/power/:power").post(|req: Request<State>| async move {
    let power = req.param("power")?;
    let controller = &req.state().controller;

    let response = if power == "on" || power == "off" {
      let (code, body) = match controller.submit_command(("pow", power)).await {
        Ok(Some(response)) => (200, json!({"response": response})),
        Ok(None) => (200, json!({"response": null})),
        Err(e) => (500, json!({"error": e.to_string()}))
      };

      Response::builder(code).body(body).build()
    } else {
      Response::builder(400).body(json!({
        "error": format!("invalid power state: {}", power)
      })).build()
    };

    Ok(response)
  });

  app.listen(opts.listen).await?;

  Ok(())
}
