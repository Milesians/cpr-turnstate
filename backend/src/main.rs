#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // stdout 用于协议通信，不能打印任何东西
    cpr_account_probe::serve(tokio::io::stdin(), tokio::io::stdout()).await
}
