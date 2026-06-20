use std::{env, io};

use time::macros::format_description;
use tracing::level_filters::LevelFilter;
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::{
    EnvFilter, filter::Directive, fmt::time::UtcTime, layer::SubscriberExt, util::SubscriberInitExt,
};

/// 初始化分层日志记录系统
///
/// 配置并启用包含文件记录和控制台输出的日志系统：
/// 1. **文件记录器**：按天轮转的日志文件（存储在指定目录）
/// 2. **控制台记录器**：带 ANSI 颜色格式的标准输出
///
/// # 参数
/// - `filename`: 日志文件前缀（不含扩展名），实际文件命名格式：`{filename}-YYYY-MM-DD.log`
/// - `directory`: 日志存储目录（可选），优先级：
///     1. 环境变量 `RUST_LOG_DIR`
///     2. 本参数指定值
///     3. 默认值 `"logs"`
/// - `default_directive`: 默认日志级别（可选），优先级：
///     1. 环境变量 `RUST_LOG`
///     2. 本参数指定值
///     3. 默认值 `INFO`
/// - `max_log_files`: 保留的日志文件数量（可选），优先级：
///     1. 环境变量 `RUST_LOG_MAX_FILES`
///     2. 本参数指定值
///     3. 默认值 `7`
///
/// # 日志特性
/// - **时间格式**:
///   - 格式：`[年]-[月]-[日]T[时]:[分]:[秒].[毫秒]Z`
///   - 示例：`2023-10-05T14:30:45.123Z`
/// - **通用字段**:
///   - 日志级别
///   - 线程名 + 线程ID
///   - 源代码位置（文件+行号）
/// - **文件输出**: 无 ANSI 转义字符
/// - **控制台输出**: 带 ANSI 颜色高亮
///
/// # 日志级别控制
/// 默认级别 `INFO`，通过环境变量覆盖：
/// ```sh
/// # 设置全局级别
/// RUST_LOG=info cargo run
/// ```
///
/// # 环境依赖
/// 自动加载 `.env` 文件（通过 `dotenvy`）
///
/// # 示例
/// ```rust
/// // 基本用法（使用所有默认值）
/// logger::init("app", None, None, None);
///
/// // 自定义目录和保留天数
/// logger::init("backend", Some("/var/log"), None, Some(30));
/// ```
pub fn init(
    filename: &str,
    directory: Option<&str>,
    default_directive: Option<Directive>,
    max_log_files: Option<usize>,
) -> WorkerGuard {
    dotenvy::dotenv().ok();

    // 获取日志目录：优先使用环境变量，其次使用参数，最后使用默认值
    let logs_dir =
        env::var("RUST_LOG_DIR").unwrap_or_else(|_| directory.unwrap_or("logs").to_string());

    // 获取日志保留数量：优先使用环境变量，其次使用参数，最后使用默认值
    let max_files = env::var("RUST_LOG_MAX_FILES")
        .ok()
        .and_then(|v| v.parse().ok())
        .or(max_log_files)
        .unwrap_or(7);

    let file_appender = tracing_appender::rolling::Builder::new()
        .filename_prefix(filename)
        .filename_suffix("log")
        .rotation(tracing_appender::rolling::Rotation::DAILY)
        .max_log_files(max_files)
        .build(logs_dir)
        .expect("initializing rolling file appender failed");
    let (non_blocking, guard) = tracing_appender::non_blocking(file_appender);
    let log_time_format =
        format_description!("[year]-[month]-[day]T[hour]:[minute]:[second].[subsecond digits:3]Z");

    // 文件输出层：无 ANSI 颜色
    let file_layer = tracing_subscriber::fmt::layer()
        .with_writer(non_blocking)
        .with_ansi(false)
        .with_target(true)
        .with_thread_names(true)
        .with_thread_ids(true)
        .with_level(true)
        .with_line_number(true)
        .with_timer(UtcTime::new(log_time_format));

    // 控制台输出层：带 ANSI 颜色
    let console_layer = tracing_subscriber::fmt::layer()
        .with_writer(io::stdout)
        .with_ansi(true)
        .with_target(true)
        .with_thread_names(true)
        .with_thread_ids(true)
        .with_level(true)
        .with_line_number(true)
        .with_timer(UtcTime::new(log_time_format));

    // 获取默认日志级别
    let default_directive = default_directive.unwrap_or_else(|| LevelFilter::INFO.into());

    tracing_subscriber::registry()
        .with(
            EnvFilter::builder()
                .with_default_directive(default_directive)
                .from_env_lossy(),
        )
        .with(file_layer)
        .with(console_layer)
        .init();
    guard
}
