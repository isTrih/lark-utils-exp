use lark_exp::client::LarkClient;
use lark_exp::config::{Config, DatabaseConfig};
use lark_exp::pipeline::workflow::run_activity_sync_workflow;
use lark_exp::xingtu::activity_config::XingtuActivityConfigRepository;
use lark_exp::xingtu::data_import::{XingtuDataImportRepository, import_pending_feishu_sources};
use lark_exp::xingtu::trace::{run_xingtu_trace_loop, run_xingtu_trace_once};
use lark_exp::xingtu::{ExportTaskOptions, XingtuClient, XingtuSession};
use sqlx::postgres::PgPoolOptions;
use std::collections::HashMap;
use std::env;
use std::path::PathBuf;
use std::time::Duration;

const DEFAULT_CONFIG_PATH: &str = "a-config-example.json";
const EXPERIMENT_INTERVAL_SECS: u64 = 300;

/// 009：测试“数据库配置 -> 定时拉取星图 spreadsheet URL -> 回写数据库”。
///
/// 常用命令：
/// - 跑一次：`cargo run --bin 009_test_xingtu_trace -- --once`
/// - 5 分钟循环：`cargo run --bin 009_test_xingtu_trace -- --loop`
/// - 自定义配置：`cargo run --bin 009_test_xingtu_trace -- --once --config configs/local.json`
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();

    let args = Args::parse(env::args().skip(1).collect())?;
    let database_config = DatabaseConfig::from_env()?;
    let pool = PgPoolOptions::new()
        .max_connections(5)
        .connect(&database_config.database_url)
        .await?;

    println!("数据库连接成功，开始执行 migration");
    sqlx::migrate!("./migrations").run(&pool).await?;

    let activity_repo = XingtuActivityConfigRepository::new(pool.clone());
    let import_repo = XingtuDataImportRepository::new(pool.clone());
    let configs = XingtuActivityConfigRepository::load_config_file(&args.config_path).await?;
    activity_repo.upsert_config_file(&configs).await?;
    println!(
        "活动配置已写入数据库：{}，配置文件：{}",
        configs.len(),
        args.config_path.display()
    );

    let options = ExportTaskOptions::default();

    if args.loop_forever {
        let xingtu = XingtuClient::new()?;
        xingtu.set_session(build_session_from_env()?).await?;
        println!("星图登录态已注入");
        println!(
            "进入实验定时拉取模式，间隔 {} 秒（5 分钟）",
            args.interval_secs
        );
        run_xingtu_trace_loop(
            activity_repo,
            xingtu,
            options,
            Duration::from_secs(args.interval_secs),
        )
        .await?;
    } else if args.sync_from_db {
        let activity_configs = activity_repo
            .list_sync_activity_configs(import_repo.clone(), chrono::Utc::now(), None)
            .await?;
        println!(
            "从数据库读取到可同步活动配置：{} 个",
            activity_configs.len()
        );
        run_activity_sync_workflow(activity_configs).await?;
    } else if args.import_pending_only {
        run_import_pending_once(&import_repo, args.pending_limit).await?;
    } else {
        let xingtu = XingtuClient::new()?;
        xingtu.set_session(build_session_from_env()?).await?;
        println!("星图登录态已注入");

        println!("开始执行单轮星图拉取");
        let result = run_xingtu_trace_once(&activity_repo, &xingtu, options).await?;
        println!("{}", serde_json::to_string_pretty(&result)?);

        if args.import_pending_after_trace {
            run_import_pending_once(&import_repo, args.pending_limit).await?;
        }
    }

    Ok(())
}

#[derive(Debug)]
struct Args {
    config_path: PathBuf,
    loop_forever: bool,
    interval_secs: u64,
    import_pending_only: bool,
    import_pending_after_trace: bool,
    sync_from_db: bool,
    pending_limit: i64,
}

impl Args {
    fn parse(args: Vec<String>) -> anyhow::Result<Self> {
        let mut config_path = PathBuf::from(DEFAULT_CONFIG_PATH);
        let mut loop_forever = false;
        let mut interval_secs = EXPERIMENT_INTERVAL_SECS;
        let mut import_pending_only = false;
        let mut import_pending_after_trace = false;
        let mut sync_from_db = false;
        let mut pending_limit = 20;
        let mut index = 0;

        while index < args.len() {
            match args[index].as_str() {
                "--once" => {
                    loop_forever = false;
                    index += 1;
                }
                "--loop" => {
                    loop_forever = true;
                    index += 1;
                }
                "--import-pending" => {
                    import_pending_only = true;
                    index += 1;
                }
                "--trace-then-import-pending" => {
                    loop_forever = false;
                    import_pending_after_trace = true;
                    index += 1;
                }
                "--sync-from-db" => {
                    sync_from_db = true;
                    index += 1;
                }
                "--config" => {
                    let Some(value) = args.get(index + 1) else {
                        anyhow::bail!("--config 需要传入配置文件路径");
                    };
                    config_path = PathBuf::from(value);
                    index += 2;
                }
                "--interval-secs" => {
                    let Some(value) = args.get(index + 1) else {
                        anyhow::bail!("--interval-secs 需要传入秒数");
                    };
                    interval_secs = value.parse()?;
                    index += 2;
                }
                "--pending-limit" => {
                    let Some(value) = args.get(index + 1) else {
                        anyhow::bail!("--pending-limit 需要传入数量");
                    };
                    pending_limit = value.parse()?;
                    index += 2;
                }
                "--help" | "-h" => {
                    print_help();
                    std::process::exit(0);
                }
                other => {
                    anyhow::bail!("未知参数：{other}");
                }
            }
        }

        if interval_secs == 0 {
            anyhow::bail!("--interval-secs 必须大于 0");
        }

        if pending_limit <= 0 {
            anyhow::bail!("--pending-limit 必须大于 0");
        }

        if loop_forever && (import_pending_only || import_pending_after_trace || sync_from_db) {
            anyhow::bail!("--loop 暂不和导入/同步参数组合使用");
        }

        let selected_extra_modes = [
            import_pending_only,
            import_pending_after_trace,
            sync_from_db,
        ]
        .into_iter()
        .filter(|selected| *selected)
        .count();

        if selected_extra_modes > 1 {
            anyhow::bail!(
                "--import-pending、--trace-then-import-pending、--sync-from-db 只能选一个"
            );
        }

        Ok(Self {
            config_path,
            loop_forever,
            interval_secs,
            import_pending_only,
            import_pending_after_trace,
            sync_from_db,
            pending_limit,
        })
    }
}

fn print_help() {
    println!(
        "009_test_xingtu_trace\n\
         用法：\n\
         cargo run --bin 009_test_xingtu_trace -- --once\n\
         cargo run --bin 009_test_xingtu_trace -- --import-pending\n\
         cargo run --bin 009_test_xingtu_trace -- --trace-then-import-pending\n\
         cargo run --bin 009_test_xingtu_trace -- --sync-from-db\n\
         cargo run --bin 009_test_xingtu_trace -- --loop\n\
         参数：\n\
         --config <path>        配置 JSON，默认 a-config-example.json\n\
         --interval-secs <n>    循环拉取间隔，默认 300 秒\n\
         --pending-limit <n>    本次最多导入多少条 pending 来源，默认 20\n\
         --once                 跑一轮后退出\n\
         --import-pending       只导入数据库中 pending 的飞书来源\n\
         --trace-then-import-pending 先拉星图链接，再导入 pending 来源\n\
         --sync-from-db         从数据库读取配置，执行直播/视频同步 pipeline\n\
         --loop                 按间隔循环拉取"
    );
}

/// 扫描并导入 pending 飞书来源。
///
/// 视频来源会额外写入 video_daily_metric / video_daily_metric_import_history；
/// 直播来源只更新 live_session，不写追踪明细。
async fn run_import_pending_once(
    repo: &XingtuDataImportRepository,
    limit: i64,
) -> anyhow::Result<()> {
    let lark = LarkClient::new(Config::from_env()?)?;
    println!("开始导入 pending 飞书来源，limit={}", limit);
    let result = import_pending_feishu_sources(repo, &lark, limit, None).await?;
    println!(
        "pending 飞书来源导入完成：成功={} 失败={} 隔离行={}",
        result.imported_sources, result.failed_sources, result.quarantined_rows
    );
    Ok(())
}

/// 从环境变量构造星图登录态。
fn build_session_from_env() -> anyhow::Result<XingtuSession> {
    let cookie =
        env::var("XINGTU_COOKIE").map_err(|_| anyhow::anyhow!("缺少环境变量 XINGTU_COOKIE"))?;
    let csrf_token = env::var("XINGTU_CSRF_TOKEN")
        .map_err(|_| anyhow::anyhow!("缺少环境变量 XINGTU_CSRF_TOKEN"))?;

    let mut session = XingtuSession::new(cookie, csrf_token);
    session.session_key = env::var("XINGTU_SESSION_KEY")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    session.user_agent = env::var("XINGTU_USER_AGENT")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    session.extra_headers = HashMap::new();

    Ok(session)
}
