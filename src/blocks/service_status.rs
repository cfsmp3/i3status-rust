//! Display the status of a service
//!
//! Right now only `systemd` is supported.
//!
//! # Configuration
//!
//! Key | Values | Default
//! ----|--------|--------
//! `driver` | Which init system is running the service. Available drivers are: `"systemd"` | `"systemd"`
//! `service` | The name of the service | **Required**
//! `active_format` | A string to customise the output of this block. See below for available placeholders. | `" $service active "`
//! `inactive_format` | A string to customise the output of this block. See below for available placeholders. | `" $service inactive "`
//! `active_state` | A valid [`State`] | [`State::Idle`]
//! `inactive_state` | A valid [`State`]  | [`State::Critical`]
//!
//! Placeholder    | Value                     | Type   | Unit
//! ---------------|---------------------------|--------|-----
//! `service`      | The name of the service   | Text   | -
//!
//! # Example
//!
//! Example using an icon:
//!
//! ```toml
//! [[block]]
//! block = "service_status"
//! service = "cups"
//! active_format = " ^icon_tea "
//! inactive_format = " no ^icon_tea "
//! ```
//!
//! Example overriding the default `inactive_state`:
//!
//! ```toml
//! [[block]]
//! block = "service_status"
//! service = "shadow"
//! active_format = ""
//! inactive_format = " Integrity of password and group files failed "
//! inactive_state = "Warning"
//! ```
//!

use super::prelude::*;
use zbus::dbus_proxy;
use zbus::PropertyStream;

#[derive(Deserialize, Debug, Default)]
#[serde(default)]
pub struct Config {
    driver: DriverType,
    service: String,
    active_format: FormatConfig,
    inactive_format: FormatConfig,
    active_state: Option<State>,
    inactive_state: Option<State>,
}

#[derive(Deserialize, Debug, SmartDefault)]
#[serde(rename_all = "snake_case")]
enum DriverType {
    #[default]
    Systemd,
}

pub async fn run(config: Config, mut api: CommonApi) -> Result<()> {
    api.event_receiver.close();

    let mut widget = Widget::new();
    let active_format = config.active_format.with_default(" $service active ")?;
    let inactive_format = config.inactive_format.with_default(" $service inactive ")?;

    let active_state = config.active_state.unwrap_or(State::Idle);
    let inactive_state = config.inactive_state.unwrap_or(State::Critical);

    let mut driver: Box<dyn Driver> = match config.driver {
        DriverType::Systemd => Box::new(SystemdDriver::new(config.service.clone()).await?),
    };

    loop {
        let service_active_state = driver.is_active().await?;

        if service_active_state {
            widget.state = active_state;
            widget.set_format(active_format.clone());
        } else {
            widget.state = inactive_state;
            widget.set_format(inactive_format.clone());
        };

        widget.set_values(map!(
            "service" =>Value::text(config.service.clone()),
        ));
        api.set_widget(&widget).await?;

        driver.wait_for_change().await?;
    }
}

#[async_trait]
trait Driver {
    async fn is_active(&self) -> Result<bool>;
    async fn wait_for_change(&mut self) -> Result<()>;
}

struct SystemdDriver {
    proxy: UnitProxy<'static>,
    active_state_changed: PropertyStream<'static, String>,
}

impl SystemdDriver {
    async fn new(service: String) -> Result<Self> {
        let dbus_conn = new_system_dbus_connection().await?;

        if !service.is_ascii() {
            return Err(Error::new(format!(
                "service name \"{service}\" must only contain ASCII characters"
            )));
        }
        let encoded_service = format!("{service}.service")
            // For each byte...
            .bytes()
            .map(|b| {
                if b.is_ascii_alphanumeric() {
                    // Just use the character as a string
                    char::from(b).to_string()
                } else {
                    // Otherwise use the hex representation of the byte preceded by an underscore
                    format!("_{b:02x}")
                }
            })
            .collect::<String>();

        let path = format!("/org/freedesktop/systemd1/unit/{encoded_service}");

        let proxy = UnitProxy::builder(&dbus_conn)
            .path(path)
            .error("Could not set path")?
            .build()
            .await
            .error("Failed to create UnitProxy")?;

        Ok(Self {
            active_state_changed: proxy.receive_active_state_changed().await,
            proxy,
        })
    }
}

#[async_trait]
impl Driver for SystemdDriver {
    async fn is_active(&self) -> Result<bool> {
        self.proxy
            .active_state()
            .await
            .error("Could not get active_state")
            .map(|state| state == "active")
    }

    async fn wait_for_change(&mut self) -> Result<()> {
        self.active_state_changed.next().await;
        Ok(())
    }
}

#[dbus_proxy(
    interface = "org.freedesktop.systemd1.Unit",
    default_service = "org.freedesktop.systemd1"
)]
trait Unit {
    #[dbus_proxy(property)]
    fn active_state(&self) -> zbus::Result<String>;
}
