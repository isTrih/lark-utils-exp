use lark_exp::server::{build_app_state, build_router};
use salvo::prelude::*;
use salvo::server::ServerHandle;
use std::time::Duration;
use std::{env, fs};
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::{EnvFilter, layer::SubscriberExt, util::SubscriberInitExt};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();
    let _log_guard = init_logging()?;
    tracing::info!(
        version = %lark_exp::version::version(),
        base_version = lark_exp::version::BASE_VERSION,
        git_commit = lark_exp::version::GIT_COMMIT,
        built_at = lark_exp::version::BUILD_TIME,
        "Lark Utils Exp 启动"
    );
    let result = run_server().await;

    if let Err(error) = &result {
        tracing::error!(error = ?error, "服务运行失败");
    }

    result
}

async fn run_server() -> anyhow::Result<()> {
    let state = build_app_state().await?;
    let router = build_router(state.clone())?;
    lark_exp::server::scheduler::spawn_scheduler(state);

    let bind_addr = env::var("SERVER_BIND_ADDR").unwrap_or_else(|_| "0.0.0.0:8080".to_string());
    let acceptor = TcpListener::new(bind_addr.clone()).bind().await;
    let server = Server::new(acceptor);
    let handle = server.handle();

    tokio::spawn(listen_shutdown_signal(handle));
    tracing::info!(
        version = %lark_exp::version::version(),
        "Salvo 服务已启动：http://{bind_addr}"
    );

    server
        .serve(Service::new(router).hoop(salvo::logging::Logger::new()))
        .await;
    Ok(())
}

fn init_logging() -> anyhow::Result<Option<WorkerGuard>> {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let console_layer = tracing_subscriber::fmt::layer();
    let log_dir = env::var("LOG_DIR")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    let (file_layer, guard) = if let Some(log_dir) = &log_dir {
        fs::create_dir_all(log_dir)?;
        let file_appender = tracing_appender::rolling::daily(log_dir, "lark-utils-exp.jsonl");
        let (writer, guard) = tracing_appender::non_blocking::NonBlockingBuilder::default()
            .lossy(false)
            .finish(file_appender);
        (
            Some(
                tracing_subscriber::fmt::layer()
                    .json()
                    .with_ansi(false)
                    .with_writer(writer),
            ),
            Some(guard),
        )
    } else {
        (None, None)
    };

    tracing_subscriber::registry()
        .with(filter)
        .with(console_layer)
        .with(file_layer)
        .init();

    if let Some(log_dir) = log_dir {
        tracing::info!(log_dir = %log_dir, "持久化日志已启用");
    } else {
        tracing::info!("未配置 LOG_DIR，仅输出控制台日志");
    }

    Ok(guard)
}

async fn listen_shutdown_signal(handle: ServerHandle) {
    if tokio::signal::ctrl_c().await.is_ok() {
        tracing::info!("收到退出信号，开始优雅关闭");
        handle.stop_graceful(Some(Duration::from_secs(30)));
    }
}
