use anyhow::Context;
use std::future::Future;
use std::pin::Pin;

/// 单个 pipeline step 返回的异步结果。
///
/// 目前项目里没有引入 async_trait，所以这里用 BoxFuture 的方式，
/// 让每个 step 可以在 trait 方法里返回异步逻辑。
pub type StepFuture<'a> = Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send + 'a>>;

/// pipeline 的单个步骤。
///
/// `Ctx` 是整条 pipeline 共享的上下文，例如视频同步会把
/// LarkClient、app_token、source_rows、upsert_plan 等状态放在上下文里。
pub trait PipelineStep<Ctx>: Send + Sync {
    /// step 名称，用于日志和错误定位。
    fn name(&self) -> &'static str;

    /// 执行 step。
    fn run<'a>(&'a self, ctx: &'a mut Ctx) -> StepFuture<'a>;
}

/// 顺序执行的轻量 pipeline。
///
/// 这个执行器故意保持很薄：只负责串联 step、打印进度和附加错误上下文。
/// 业务状态和具体逻辑都放在各自的 context/step 中，方便后续按同步类型扩展。
pub struct Pipeline<Ctx> {
    name: &'static str,
    steps: Vec<Box<dyn PipelineStep<Ctx>>>,
}

impl<Ctx> Pipeline<Ctx> {
    /// 创建一条新的 pipeline。
    pub fn new(name: &'static str) -> Self {
        Self {
            name,
            steps: Vec::new(),
        }
    }

    /// 追加一个 step。
    pub fn add_step(mut self, step: impl PipelineStep<Ctx> + 'static) -> Self {
        self.steps.push(Box::new(step));
        self
    }

    /// 按添加顺序执行所有 step。
    pub async fn run(&self, ctx: &mut Ctx) -> anyhow::Result<()> {
        tracing::info!("开始执行 pipeline：{}", self.name);

        for (index, step) in self.steps.iter().enumerate() {
            let step_number = index + 1;
            tracing::info!(
                "开始执行 step {}/{}：{}",
                step_number,
                self.steps.len(),
                step.name()
            );

            step.run(ctx).await.with_context(|| {
                format!(
                    "pipeline `{}` 的 step `{}` 执行失败",
                    self.name,
                    step.name()
                )
            })?;

            tracing::info!("step 完成：{}", step.name());
        }

        tracing::info!("pipeline 完成：{}", self.name);
        Ok(())
    }
}
