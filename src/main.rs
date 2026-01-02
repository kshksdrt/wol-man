use anyhow::Result;
use esp_idf_svc::eventloop::EspSystemEventLoop;
use esp_idf_svc::hal::modem::Modem;
use esp_idf_svc::hal::prelude::*;
use esp_idf_svc::http::client::{Configuration as HttpConfig, EspHttpConnection};
use esp_idf_svc::http::Method;
use esp_idf_svc::nvs::EspDefaultNvsPartition;
use esp_idf_svc::wifi::{ClientConfiguration, Configuration, EspWifi};
use log::*;
use serde::Deserialize;
use std::net::UdpSocket;
use std::thread;
use std::time::Duration;

// Network configuration
const SSID: &str = "";
const PASS: &str = "";

// Telegram communication configuration
const TELEGRAM_TOKEN: &str = "";
const AUTHORIZED_USERS: [i64; 1] = [];

// Wake-on-LAN configuration
const TARGET_MAC: [u8; 6] = [];

#[derive(Debug, Deserialize)]
struct TelegramResponse {
    ok: bool,
    result: Vec<Update>,
}

#[derive(Debug, Deserialize)]
struct Update {
    update_id: u64,
    message: Option<Message>,
}

#[derive(Debug, Deserialize)]
struct Message {
    text: Option<String>,
    chat: Chat,
}

#[derive(Debug, Deserialize)]
struct Chat {
    id: i64,
}

fn main() -> Result<()> {
    // 1. Initializing (required to be initialized in the main thred)
    esp_idf_svc::sys::link_patches();
    esp_idf_svc::log::EspLogger::initialize_default();
    let peripherals = Peripherals::take()?;
    let sys_loop = EspSystemEventLoop::take()?;
    let nvs = EspDefaultNvsPartition::take()?;

    let wifi = connect_to_wifi(peripherals.modem, sys_loop, nvs)?;

    // Wait for IP assignment
    while !wifi.is_up()? {
        let _config = wifi.get_configuration()?;
        info!("Waiting for station to connect...");
        thread::sleep(Duration::from_millis(1000));
    }
    info!("WiFi Connected!");

    let mut offset: u64 = 0;

    loop {
        let url = format!(
            "https://api.telegram.org/bot{}/getUpdates?offset={}&timeout=30",
            TELEGRAM_TOKEN, offset
        );

        let connection = EspHttpConnection::new(&HttpConfig {
            timeout: Some(Duration::from_secs(40)),
            crt_bundle_attach: Some(esp_idf_svc::sys::esp_crt_bundle_attach),
            ..Default::default()
        })?;

        let mut client = embedded_svc::http::client::Client::wrap(connection);

        info!("Polling Telegram... (offset: {})", offset);

        let response_result = client
            .request(Method::Get, &url, &[])
            .and_then(|request| request.submit());

        match response_result {
            Ok(mut response) => {
                if response.status() == 200 {
                    let mut body = String::new();
                    let mut buf = [0u8; 1024];

                    loop {
                        match response.read(&mut buf) {
                            Ok(0) => break, // EOF
                            Ok(size) => body.push_str(&String::from_utf8_lossy(&buf[..size])),
                            Err(_) => break,
                        }
                    }

                    if let Ok(updates) = serde_json::from_str::<TelegramResponse>(&body) {
                        for update in updates.result {
                            offset = update.update_id + 1;
                            if let Some(msg) = update.message {
                                if !AUTHORIZED_USERS.contains(&msg.chat.id) {
                                    info!("Unauthorized access attempt from ID: {}", msg.chat.id);
                                    continue;
                                }
                                if let Some(text) = msg.text {
                                    info!("Received message: {}", text);
                                    if text.trim() == "/health" {
                                        send_telegram_message(msg.chat.id, "Ready!");
                                    }
                                    if text.trim() == "/wake" {
                                        send_wol_packet();
                                        send_telegram_message(msg.chat.id, "Success!");
                                    }
                                }
                            }
                        }
                    } else {
                        error!("Failed to parse JSON response");
                    }
                } else {
                    error!("Telegram error status: {}", response.status());
                }
            }
            Err(e) => error!("HTTP Request failed: {}", e),
        }

        thread::sleep(Duration::from_secs(1));
    }
}

fn connect_to_wifi<'a>(
    modem: Modem,
    sys_loop: EspSystemEventLoop,
    nvs: EspDefaultNvsPartition,
) -> Result<EspWifi<'a>> {
    let mut wifi = EspWifi::new(modem, sys_loop, Some(nvs))?;

    wifi.set_configuration(&Configuration::Client(ClientConfiguration {
        ssid: SSID.try_into().unwrap(),
        password: PASS.try_into().unwrap(),
        ..Default::default()
    }))?;

    wifi.start()?;
    wifi.connect()?;

    Ok(wifi)
}

fn send_telegram_message(chat_id: i64, text: &str) {
    let url = format!("https://api.telegram.org/bot{}/sendMessage", TELEGRAM_TOKEN);

    let payload = serde_json::json!({
        "chat_id": chat_id,
        "text": text
    });

    let config = HttpConfig {
        crt_bundle_attach: Some(esp_idf_svc::sys::esp_crt_bundle_attach),
        ..Default::default()
    };

    if let Ok(connection) = EspHttpConnection::new(&config) {
        let mut client = embedded_svc::http::client::Client::wrap(connection);

        let body = payload.to_string();
        let headers = [("Content-Type", "application/json")];

        if let Ok(mut request) = client.request(Method::Post, &url, &headers) {
            if request.write(body.as_bytes()).is_ok() {
                if let Ok(response) = request.submit() {
                    info!("Reply sent status: {}", response.status());
                }
            }
        }
    }
}

fn send_wol_packet() {
    info!("Sending Wake-on-LAN packet...");
    let mut packet = vec![0xFF; 6];
    for _ in 0..16 {
        packet.extend_from_slice(&TARGET_MAC);
    }

    match UdpSocket::bind("0.0.0.0:0") {
        Ok(socket) => {
            socket.set_broadcast(true).ok();
            if let Err(e) = socket.send_to(&packet, "255.255.255.255:9") {
                error!("Failed to send WoL packet: {}", e);
            } else {
                info!("WoL packet sent successfully!");
            }
        }
        Err(e) => error!("Failed to bind UDP socket: {}", e),
    }
}
