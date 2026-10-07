fn main() {
    println!(
        "{}",
        serde_json::to_string_pretty(&kunlun_devtools_protocol::schema()).unwrap()
    );
}
