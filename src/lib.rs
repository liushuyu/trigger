// Logging support
#[macro_use]
extern crate log;

// Config parser (YAML)
extern crate yaml_rust;

// Webhook listener
extern crate hyper;
extern crate rifling;

// Run shell commands
extern crate run_script;

use std::collections::HashMap;
use std::error::Error;
use std::fs::File;
use std::io::prelude::*;
use std::io::BufReader;
use std::net::SocketAddr;
use std::thread;

use hyper::rt::{run, Future};
use hyper::Server;
use run_script::ScriptOptions;
use yaml_rust::{Yaml, YamlLoader};

use rifling::hook::HookFunc;
use rifling::{Constructor, Delivery, Hook};

macro_rules! get_value {
    ($source:expr) => {
        match $source {
            Some(string) => string.as_str(),
            None => "unknown",
        }
    };
}

// Some constant
/// Name of the settings section in configuration file
const SETTINGS: &str = "settings";
/// Name of the events section in configuration file
const EVENTS: &str = "events";
/// Name of the common part inside events section in configuration file
const EVENTS_COMMON: &str = "common";
/// Name of the `else` part inside events section in configuration file
const EVENTS_ELSE: &str = "else";
/// Name of the `all` part inside events section in configuration file
const EVENTS_ALL: &str = "all";

#[derive(Clone)]
/// Handler of the deliveries
pub struct Handler {
    config: Yaml,
}

/// Handler of the deliveries
impl Handler {
    /// Create a new instance from given configuration
    fn new(config: Yaml) -> Handler {
        Handler { config }
    }

    /// Prepare command from information of the delivery
    fn process_commands(&self, event: &str, delivery: &Delivery) -> Option<String> {
        let common_command = self.config[EVENTS][EVENTS_COMMON].as_str().unwrap_or("");
        if let Some(command) = self.config[EVENTS][event].as_str() {
            let mut exec = String::from(command);
            exec = format!("{}\n{}", &common_command, &exec);
            // Replace placeholders in commands
            exec = exec.replace("{id}", get_value!(&delivery.id));
            exec = exec.replace("{event}", get_value!(&delivery.event));
            exec = exec.replace("{signature}", get_value!(&delivery.signature));
            exec = exec.replace("{payload}", get_value!(&delivery.unparsed_payload));
            exec = exec.replace("{request_body}", get_value!(&delivery.request_body));
            Some(exec)
        } else {
            None
        }
    }
}

impl HookFunc for Handler {
    /// Handle the delivery
    fn run(&self, delivery: &Delivery) {
        let id = get_value!(&delivery.id);
        let event = get_value!(&delivery.event);
        info!("Received \"{}\" event with ID \"{}\"", &event, &id);

        // Prepare the commands
        let mut commands_all: HashMap<String, Option<String>> = HashMap::new();

        // Prepare commands in `all` section
        commands_all.insert(
            EVENTS_ALL.into(),
            self.process_commands(EVENTS_ALL, &delivery),
        );

        // Prepare commands matching the event
        if let Some(command) = self.process_commands(event, &delivery) {
            commands_all.insert(event.into(), Some(command));
        } else {
            commands_all.insert(
                EVENTS_ELSE.into(),
                self.process_commands(EVENTS_ELSE, &delivery),
            );
        }

        // Execute the commands
        for (section_name, command) in commands_all {
            if let Some(exec) = command {
                info!("Running commands in \"{}\" section", event);
                debug!("Parsed command: {}", &exec);
                let mut options = ScriptOptions::new();
                options.capture_output = self.config[SETTINGS]["capture_output"]
                    .as_bool()
                    .unwrap_or(false);
                options.exit_on_error = self.config[SETTINGS]["exit_on_error"]
                    .as_bool()
                    .unwrap_or(false);
                options.print_commands = self.config[SETTINGS]["print_commands"]
                    .as_bool()
                    .unwrap_or(false);
                debug!("Executor option: {:#?}", &options);
                let args = vec![];
                thread::spawn(move || {
                    run_script::run(&exec.as_str(), &args, &options)
                        .expect("Failed to execute command");
                    info!("Commands in \"{}\" section exited", &section_name);
                });
            }
        }
        info!("Returning 200");
    }
}

/// Start the server from given config file path
pub fn start(config_filename: &str) -> Result<(), Box<Error>> {
    info!("Starting up...");

    // Read config (from `trigger.yaml`)
    let mut config_content = String::new();
    let config_file = File::open(config_filename)?;
    let mut buf_reader = BufReader::new(config_file);
    buf_reader.read_to_string(&mut config_content)?;
    debug!(
        "Got config:\n\"\"\"\n{}\n\"\"\"\nfrom file {}",
        config_content, config_filename
    );

    let config = YamlLoader::load_from_str(config_content.as_str())?[0].clone();
    debug!("Config parsed: {:?}", config);

    // Prepare secret
    let secret = if let Some(secret) = config[SETTINGS]["secret"].as_str() {
        Some(String::from(secret))
    } else {
        None
    };

    // Setup handler
    let handler = Handler::new(config.clone());
    let mut cons = Constructor::new();
    let hook = Hook::new("*", secret, handler);
    cons.register(hook);

    // Setup server
    let addr: SocketAddr = config[SETTINGS]["host"]
        .as_str()
        .expect("Unable to read host address")
        .parse()
        .expect("Unable to parse host address");
    let ip_type = if addr.is_ipv4() { "IPv4" } else { "IPv6" };
    info!(
        "Listening on {} address {}:{}",
        ip_type,
        &addr.ip(),
        &addr.port()
    );
    let server = Server::bind(&addr)
        .serve(cons)
        .map_err(|e| error!("Error: {:?}", e));
    info!("Started");

    // Link start!
    run(server);
    Ok(())
}
