use crate::client::LarkClient;
use anyhow::{Context, anyhow};
use open_lark::communication::im::v1::message::create::{CreateMessageBody, CreateMessageRequest};
use open_lark::communication::im::v1::message::delete::DeleteMessageRequest;
pub use open_lark::communication::im::v1::message::models::ReceiveIdType;
use serde_json::{Value, json};

/// 飞书消息发送结果。
///
/// `raw_response` 保留完整响应，便于调试飞书返回结构变化；
/// `message_id` 只提取业务最常用的消息 ID。
#[derive(Debug, Clone)]
pub struct SendMessageResult {
    pub message_id: Option<String>,
    pub raw_response: Value,
}

/// 飞书消息接收者配置。
///
/// 后续任务调度器可以从数据库或任务配置中读取这份配置，
/// 再传给 IM 发送方法；这里不依赖环境变量。
#[derive(Debug, Clone)]
pub struct MessageReceiver {
    pub receive_id_type: ReceiveIdType,
    pub receive_id: String,
    pub uuid: Option<String>,
}

/// 审核通知模板中的单期审核信息。
///
/// 字段名会被组装成卡片模板要求的变量：
/// `Period`、`video_nums`、`live_nums`、`audit_table_url`。
#[derive(Debug, Clone)]
pub struct AuditInfoItem {
    pub period: String,
    pub video_nums: String,
    pub live_nums: String,
    pub audit_table_url: String,
}

impl AuditInfoItem {
    /// 创建一条审核信息。
    ///
    /// 数量字段按字符串保存，和飞书卡片模板变量示例保持一致；
    /// 调度器传入数字时也可以直接用 `usize/u64` 等实现了 `ToString` 的类型。
    pub fn new(
        period: impl Into<String>,
        video_nums: impl ToString,
        live_nums: impl ToString,
        audit_table_url: impl Into<String>,
    ) -> Self {
        Self {
            period: period.into(),
            video_nums: video_nums.to_string(),
            live_nums: live_nums.to_string(),
            audit_table_url: audit_table_url.into(),
        }
    }
}

/// 审核通知卡片配置。
///
/// 这里的字段严格对应飞书模板变量：
/// - `auditor_ids`
/// - `audit_info`
/// - `project_name`
#[derive(Debug, Clone)]
pub struct AuditReviewNoticeConfig {
    pub template_id: String,
    pub auditor_ids: String,
    pub project_name: String,
    pub audit_info: Vec<AuditInfoItem>,
}

impl AuditReviewNoticeConfig {
    /// 使用外部配置的模板和项目级审核人创建配置。
    pub fn with_template(
        template_id: impl Into<String>,
        auditor_ids: impl Into<String>,
        project_name: impl Into<String>,
        audit_info: Vec<AuditInfoItem>,
    ) -> Self {
        Self {
            template_id: template_id.into(),
            auditor_ids: auditor_ids.into(),
            project_name: project_name.into(),
            audit_info,
        }
    }
}

/// 星图及工作流错误通知模板配置。
///
/// 模板变量严格对应飞书卡片模板：
/// - `ops_ids`：需要通知的人的 ID，用逗号分隔
/// - `project_name`：项目名称
/// - `error_type`：错误类型
/// - `error_detail`：包含账号、时间和原始原因的错误详情
#[derive(Debug, Clone)]
pub struct XingtuLoginNoticeConfig {
    pub template_id: String,
    pub ops_ids: String,
    pub project_name: String,
    pub error_type: String,
    pub error_detail: String,
}

impl XingtuLoginNoticeConfig {
    pub fn new(
        template_id: impl Into<String>,
        ops_ids: impl Into<String>,
        project_name: impl Into<String>,
        error_type: impl Into<String>,
        error_detail: impl Into<String>,
    ) -> Self {
        Self {
            template_id: template_id.into(),
            ops_ids: ops_ids.into(),
            project_name: project_name.into(),
            error_type: error_type.into(),
            error_detail: error_detail.into(),
        }
    }
}

/// 飞书 IM 消息发送器。
///
/// 这里收敛所有消息发送通用逻辑：
/// - `send_message`：底层消息 API 封装
/// - `send_card`：交互式卡片消息封装
/// - `send_audit_review_notice`：审核通知模板卡片封装
pub struct FeishuImClient<'a> {
    lark: &'a LarkClient,
}

impl<'a> FeishuImClient<'a> {
    pub fn new(lark: &'a LarkClient) -> Self {
        Self { lark }
    }

    /// 发送飞书交互式卡片。
    ///
    /// 飞书 `interactive` 消息要求 content 是卡片 JSON 字符串，
    /// 所以这里统一把 `serde_json::Value` 转成字符串。
    pub async fn send_card(
        &self,
        receive_id_type: ReceiveIdType,
        receive_id: &str,
        card: Value,
        uuid: Option<String>,
    ) -> anyhow::Result<SendMessageResult> {
        self.send_message(
            receive_id_type,
            receive_id,
            "interactive",
            card.to_string(),
            uuid,
        )
        .await
    }

    /// 按接收者配置发送飞书交互式卡片。
    pub async fn send_card_to_receiver(
        &self,
        receiver: &MessageReceiver,
        card: Value,
    ) -> anyhow::Result<SendMessageResult> {
        self.send_card(
            receiver.receive_id_type,
            &receiver.receive_id,
            card,
            receiver.uuid.clone(),
        )
        .await
    }

    /// 发送飞书消息。
    ///
    /// 业务层传入接收者、消息类型、content 和 uuid；
    /// 本方法负责调用 openlark 的 `CreateMessageRequest` 并统一返回结果。
    pub async fn send_message(
        &self,
        receive_id_type: ReceiveIdType,
        receive_id: &str,
        msg_type: &str,
        content: String,
        uuid: Option<String>,
    ) -> anyhow::Result<SendMessageResult> {
        let response = CreateMessageRequest::new(self.lark.raw().config().clone())
            .receive_id_type(receive_id_type)
            .execute(CreateMessageBody {
                receive_id: receive_id.to_string(),
                msg_type: msg_type.to_string(),
                content,
                uuid,
            })
            .await
            .context("发送飞书消息失败")?;

        Ok(SendMessageResult {
            message_id: extract_message_id(&response),
            raw_response: response,
        })
    }

    /// 撤回一条由当前飞书应用发送的消息。
    pub async fn recall_message(&self, message_id: &str) -> anyhow::Result<()> {
        if message_id.trim().is_empty() {
            return Err(anyhow!("message_id 不能为空"));
        }

        DeleteMessageRequest::new(self.lark.raw().config().clone())
            .message_id(message_id.trim())
            .execute()
            .await
            .with_context(|| format!("撤回飞书消息失败：message_id={message_id}"))?;
        Ok(())
    }

    /// 发送“通知审核”模板卡片。
    ///
    /// 卡片变量只包含模板需要的 `auditor_ids`、`audit_info` 和 `project_name`，
    /// 避免业务 workflow 误传其它自定义字段。
    pub async fn send_audit_review_notice(
        &self,
        receiver: &MessageReceiver,
        config: &AuditReviewNoticeConfig,
    ) -> anyhow::Result<SendMessageResult> {
        let card = build_audit_review_template_card(config);
        self.send_card_to_receiver(receiver, card).await
    }

    /// 发送星图及工作流错误模板卡片。
    pub async fn send_xingtu_login_notice(
        &self,
        receiver: &MessageReceiver,
        config: &XingtuLoginNoticeConfig,
    ) -> anyhow::Result<SendMessageResult> {
        let card = build_xingtu_login_notice_template_card(config);
        self.send_card_to_receiver(receiver, card).await
    }
}

/// 构建飞书模板卡片。
pub fn build_template_card(template_id: &str, template_variable: Value) -> Value {
    json!({
        "type": "template",
        "data": {
            "template_id": template_id,
            "template_variable": template_variable
        }
    })
}

/// 构建“通知审核”模板卡片。
pub fn build_audit_review_template_card(config: &AuditReviewNoticeConfig) -> Value {
    build_template_card(
        &config.template_id,
        json!({
            "auditor_ids": config.auditor_ids,
            "audit_info": config.audit_info.iter().map(build_audit_info_value).collect::<Vec<_>>(),
            "project_name": config.project_name
        }),
    )
}

/// 构建星图及工作流错误模板卡片。
pub fn build_xingtu_login_notice_template_card(config: &XingtuLoginNoticeConfig) -> Value {
    build_template_card(
        &config.template_id,
        json!({
            "ops_ids": config.ops_ids,
            "project_name": config.project_name,
            "error_type": config.error_type,
            "error_detail": config.error_detail
        }),
    )
}

/// 将内部结构转换为卡片模板要求的变量字段。
fn build_audit_info_value(item: &AuditInfoItem) -> Value {
    json!({
        "Period": item.period,
        "video_nums": item.video_nums,
        "live_nums": item.live_nums,
        "audit_table_url": item.audit_table_url
    })
}

/// 解析飞书接收者 ID 类型。
pub fn parse_receive_id_type(value: &str) -> anyhow::Result<ReceiveIdType> {
    match value.trim().to_ascii_lowercase().as_str() {
        "open_id" => Ok(ReceiveIdType::OpenId),
        "union_id" => Ok(ReceiveIdType::UnionId),
        "user_id" => Ok(ReceiveIdType::UserId),
        "email" => Ok(ReceiveIdType::Email),
        "chat_id" => Ok(ReceiveIdType::ChatId),
        other => Err(anyhow!(
            "不支持的 receive_id_type `{}`，可选值：open_id / union_id / user_id / email / chat_id",
            other
        )),
    }
}

/// 从飞书返回中提取 message_id。
///
/// 当前 openlark 的发送接口已经返回 response.data；这里仍然做递归查找，
/// 是为了兼容后续 SDK 返回结构变化。
pub fn extract_message_id(response: &Value) -> Option<String> {
    match response {
        Value::Object(map) => {
            if let Some(value) = map.get("message_id").and_then(Value::as_str) {
                return Some(value.to_string());
            }

            map.values().find_map(extract_message_id)
        }
        Value::Array(values) => values.iter().find_map(extract_message_id),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audit_review_template_card_uses_expected_variables() {
        let card = build_audit_review_template_card(&AuditReviewNoticeConfig::with_template(
            "template-test",
            "auditor-a,auditor-b",
            "ROK",
            vec![AuditInfoItem::new(
                "6月第十三期",
                12,
                3,
                "[6月第十三期](https://example.com/audit)",
            )],
        ));

        assert_eq!(card["type"], "template");
        assert_eq!(card["data"]["template_id"], "template-test");
        assert_eq!(
            card["data"]["template_variable"]["auditor_ids"],
            "auditor-a,auditor-b"
        );
        assert_eq!(
            card["data"]["template_variable"]["audit_info"][0]["Period"],
            "6月第十三期"
        );
        assert_eq!(
            card["data"]["template_variable"]["audit_info"][0]["video_nums"],
            "12"
        );
        assert_eq!(
            card["data"]["template_variable"]["audit_info"][0]["live_nums"],
            "3"
        );
        assert_eq!(card["data"]["template_variable"]["project_name"], "ROK");
    }

    #[test]
    fn receive_id_type_parser_accepts_chat_id() {
        assert_eq!(
            parse_receive_id_type("chat_id").unwrap(),
            ReceiveIdType::ChatId
        );
    }

    #[test]
    fn audit_review_template_card_uses_configured_auditors() {
        let card = build_audit_review_template_card(&AuditReviewNoticeConfig::with_template(
            "tpl-test",
            "user-a,user-b",
            "ROK",
            vec![AuditInfoItem::new("测试期", 1, 2, "https://example.com")],
        ));

        assert_eq!(card["data"]["template_id"], "tpl-test");
        assert_eq!(
            card["data"]["template_variable"]["auditor_ids"],
            "user-a,user-b"
        );
    }

    #[test]
    fn xingtu_error_notice_card_uses_error_variables() {
        let card = build_xingtu_login_notice_template_card(&XingtuLoginNoticeConfig::new(
            "login-template-test",
            "user-a,user-b",
            "ROK",
            "星图登陆状态失效",
            "星图登陆态失效，请及时更新星图登陆态。\n账号ID:123\n2026:07:30 12:00:00",
        ));
        let variables = &card["data"]["template_variable"];

        assert_eq!(variables["ops_ids"], "user-a,user-b");
        assert_eq!(variables["project_name"], "ROK");
        assert_eq!(variables["error_type"], "星图登陆状态失效");
        assert_eq!(
            variables["error_detail"],
            "星图登陆态失效，请及时更新星图登陆态。\n账号ID:123\n2026:07:30 12:00:00"
        );
    }
}
