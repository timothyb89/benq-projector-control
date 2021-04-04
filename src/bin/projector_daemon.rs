use std::sync::Arc;
use std::time::Duration;

use astro_dnssd::{txt::TXTRecord, register::DNSServiceBuilder};
use benq_control::{Command, ProjectorControl};
use color_eyre::eyre::{Result, Context, ContextCompat, eyre};
use futures::try_join;
use log::*;
use structopt::StructOpt;
use tokio::{task, sync::RwLock};
use tide::{Body, Request, Response};
use tide::prelude::*;
use url::Url;

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

  /// port and protocol to listen on
  #[structopt(
    long, short,
    default_value = "http://0.0.0.0:8084",
    env = "PROJECTOR_LISTEN"
  )]
  listen: String,

  /// Name to advertise via mDNS
  #[structopt(
    long, short,
    default_value = "Projector Control",
    env = "PROJECTOR_MDNS_NAME"
  )]
  mdns_name: String,

  /// Unique ID for clients like Home Assistant
  ///
  /// If unset, uses the MAC address of the first non-local interface
  #[structopt(
    long, short,
    env = "PROJECTOR_UNIQUE_ID"
  )]
  unique_id: Option<String>
}

#[derive(Debug, Serialize)]
struct ProjectorStatus {
  state: ProjectorState,

  unique_id: String,
  model: String,
}

#[derive(Debug, Serialize)]
#[serde(tag = "power", rename_all = "lowercase")]
enum ProjectorState {
  On {
    source: String,
    volume: u8,
    max_volume: u8,
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

type WrappedProjectorStatus = Arc<RwLock<ProjectorStatus>>;

async fn fetch_status(
  controller: &ProjectorControl,
  prev_power_state: bool,
  unique_id: impl Into<String>
) -> Result<ProjectorStatus> {
  let model = controller.submit_command("modelname")
    .await?
    .map(|m| m.trim_start_matches("MODELNAME=").to_string())
    .unwrap_or(String::from("Unknown"));

  let power = controller.submit_command("pow").await?;
  if let Some("POW=ON") = power.as_deref() {
    if !prev_power_state {
      // looks like the projector turned on, send a sleep command to block
      // processing for a bit
      // note: this may have been us and will add on to the existing safety
      // sleep, oh well
      controller.submit_command(Command::Sleep(Duration::from_secs(5)));
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

    Ok(ProjectorStatus {
      model,
      unique_id: unique_id.into(),
      state: ProjectorState::On {
        source,
        volume,
        max_volume: 20,
        muted
      }
    })
  } else {
    if prev_power_state {
      // looks like the projector turned off, send a sleep command to block
      // processing for a bit
      // note: this may have been us and will add on to the existing safety
      // sleep, oh well
      // note: the projector takes longer to power off, so sleep 60s rather than
      // 30
      controller.submit_command(Command::Sleep(Duration::from_secs(20)));
    }

    Ok(ProjectorStatus {
      model,
      state: ProjectorState::Off,
      unique_id: unique_id.into()
    })
  }
}

async fn update_state(controller: &ProjectorControl, status: &WrappedProjectorStatus) -> Result<()> {
  let (unique_id, prev_power_state) = {
    let prev_status = status.read().await;

    (prev_status.unique_id.clone(), prev_status.state.is_on())
  };

  let new_state = match fetch_status(controller, prev_power_state, unique_id).await {
    Ok(state) => state,
    Err(e) => {
      warn!("state refresh failed, sleeping 30s to prevent interface crash");
      controller.submit_command(Command::Sleep(Duration::from_secs(30)));
      return Err(e).context("fetching updated projector state");
    }
  };

  let mut w = status.write().await;
  *w = new_state;

  Ok(())
}

async fn update_state_task(controller: &ProjectorControl, state: WrappedProjectorStatus) {
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
  projector_status: WrappedProjectorStatus,
  controller: Arc<ProjectorControl>,
}

fn register_dnssd(listen: &str, name: &str, unique_id: &str) -> Result<()> {
  let url = Url::parse(listen).context("parsing listen url")?;
  let port = url.port().unwrap_or(80);

  let mut txt = TXTRecord::new();
  let _ = txt.insert("id", Some(unique_id));
  let mut service = DNSServiceBuilder::new("_benq_projector._tcp")
    .with_port(port)
    .with_name(name)
    .with_txt_record(txt)
    .build()
    .unwrap();

  debug!("attempting to register mdns service with name {}", name);
  let _result = service.register(|reply| match reply {
    Ok(reply) => info!("mdns registration reply: {:?}", reply),
    Err(e) => error!("mdns registration error: {:?}", e),
  });

  loop {
    service.process_result();
  }
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

  let unique_id = if let Some(unique_id) = opts.unique_id {
    unique_id.clone()
  } else {
    match mac_address::get_mac_address() {
      Ok(Some(addr)) => addr.to_string(),
      Ok(None) => return Err(
        eyre!("no usable unique id found, set one manually with --unique-id")
      ),
      Err(e) => return Err(
        eyre!(e)
          .wrap_err("could not determine a unique id, set one manually with --unique-id")
      )
    }
  };

  debug!("unique id: {}", unique_id);

  let mdns_listen = opts.listen.clone();
  let mdns_name = opts.mdns_name.clone();
  let mdns_unique_id = unique_id.clone();

  std::thread::spawn(move || {
    info!("started mdns thread");
    if let Err(e) = register_dnssd(&mdns_listen, &mdns_name, &mdns_unique_id) {
      error!("unable to register server via mdns: {}", e);
    }
  });

  let serial_port = serialport::new(&opts.device, opts.baud_rate)
    .timeout(Duration::from_millis(100))
    .open()?;
  let controller = Arc::new(ProjectorControl::new(serial_port));
  let projector_status = Arc::new(RwLock::new(ProjectorStatus {
    model: "Unknown".to_string(),
    state: ProjectorState::Invalid,
    unique_id
  }));

  // spawn a task to continuously refresh the projector's status
  let refresh_controller = Arc::clone(&controller);
  let refresh_status = Arc::clone(&projector_status);
  task::spawn(async move {
    update_state_task(&refresh_controller, refresh_status).await;
  });

  let state = State {
    projector_status: Arc::clone(&projector_status),
    controller: Arc::clone(&controller),
  };

  let mut app = tide::with_state(state);
  app.at("/status").get(|req: Request<State>| async move {
    let projector_status = req.state().projector_status.read().await;

    Ok(Body::from_json(&*projector_status)?)
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
    let power = req.param("power")?.to_lowercase();
    let controller = &req.state().controller;

    let response = if power == "on" || power == "off" {
      let (code, body) = match controller.submit_command(("pow", power.as_str())).await {
        Ok(Some(response)) => (200, json!({"response": response})),
        Ok(None) => (200, json!({"response": null})),
        Err(e) => (500, json!({"error": e.to_string()}))
      };

      // if successful, update the state directly - the processing thread will
      // be paused for quite a while but we can safely assume it's (turning) off
      if code == 200 && power == "off" {
        let mut status = req.state().projector_status.write().await;
        status.state = ProjectorState::Off;
      }

      Response::builder(code).body(body).build()
    } else {
      Response::builder(400).body(json!({
        "error": format!("invalid power state: {}", power)
      })).build()
    };

    Ok(response)
  });

  app.at("/source/:source").post(|req: Request<State>| async move {
    let source = req.param("source")?.to_lowercase();
    let controller = &req.state().controller;

    let response = if let "rgb" | "hdmi" | "hdmi2" = source.as_str() {
      let (code, body) = match controller.submit_command(("sour", source)).await {
        Ok(Some(response)) => (200, json!({"response": response})),
        Ok(None) => (200, json!({"response": null})),
        Err(e) => (500, json!({"error": e.to_string()}))
      };

      // kick off a state update right away to reflect the new status
      if let Err(e) = update_state(controller, &req.state().projector_status).await {
        warn!("(post source) state update failed: {:?}", e);
      }

      Response::builder(code).body(body).build()
    } else {
      Response::builder(400).body(json!({
        "error": format!("invalid source: {}", source)
      })).build()
    };

    Ok(response)
  });

  app.at("/volume/:volume").post(|req: Request<State>| async move {
    let volume = req.param("volume")?.to_lowercase();
    let controller = &req.state().controller;

    let (code, body) = match volume.parse::<u8>() {
      Ok(v @ 0..=20) => {
        let (code, body) = match controller.submit_command(("vol", v.to_string())).await {
          Ok(Some(response)) => (200, json!({"response": response})),
          Ok(None) => (200, json!({"response": null})),
          Err(e) => (500, json!({"error": e.to_string()}))
        };

        // kick off a state update right away to reflect the new status
        if let Err(e) = update_state(controller, &req.state().projector_status).await {
          warn!("(post source) state update failed: {:?}", e);
        }

        (code, body)
      },
      Ok(_) => (400, json!({
        "error": format!("volume out of range: {}", volume)
      })),
      Err(_) => (400, json!({
        "error": format!("invalid volume: {}", volume)
      }))
    };

    Ok(Response::builder(code).body(body).build())
  });

  app.at("/mute/:mute").post(|req: Request<State>| async move {
    let mute = req.param("mute")?.to_lowercase();
    let controller = &req.state().controller;

    let (code, body) = if let "on" | "off" = mute.as_str() {
      let (code, body) = match controller.submit_command(("mute", mute)).await {
        Ok(Some(response)) => (200, json!({"response": response})),
        Ok(None) => (200, json!({"response": null})),
        Err(e) => (500, json!({"error": e.to_string()}))
      };

      // kick off a state update right away to reflect the new status
      if let Err(e) = update_state(controller, &req.state().projector_status).await {
        warn!("(post source) state update failed: {:?}", e);
      }

      (code, body)
    } else {
      (400, json!({
        "error": format!("invalid mute state: {}", mute)
      }))
    };

    Ok(Response::builder(code).body(body).build())
  });

  app.listen(opts.listen).await?;

  Ok(())
}
