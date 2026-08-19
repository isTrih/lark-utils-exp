use crate::client::LarkClient;
use crate::pipeline::activity::ActivityTableSyncConfig;
use crate::pipeline::sync::FieldValueRules;
use crate::pipeline::table_sync::{TableSyncConfig, run_table_sync_pipeline};

/// 直播同步业务固定的唯一键字段。
///
/// 每期活动会变的是 URL 和表 ID；唯一键属于直播同步业务规则，不由活动配置传入。
pub const LIVE_UNIQUE_KEY_FIELD: &str = "直播间ID";

/// 运行直播同步 pipeline。
pub async fn run_live_sync(
    lark: LarkClient,
    activity_config: ActivityTableSyncConfig,
) -> anyhow::Result<()> {
    run_table_sync_pipeline(
        lark,
        build_table_sync_config(activity_config),
        live_field_rules(),
    )
    .await
}

/// 把每期活动可变配置转换成直播同步内部配置。
fn build_table_sync_config(activity_config: ActivityTableSyncConfig) -> TableSyncConfig {
    TableSyncConfig {
        pipeline_name: "live_sync",
        bitable_url: activity_config.bitable_url,
        spreadsheet_url: activity_config.spreadsheet_url,
        source_mode: activity_config.source_mode,
        manual_table_id: activity_config.manual_table_id,
        main_table_id: activity_config.main_table_id,
        audit_table_id: activity_config.audit_table_id,
        unique_key_field: LIVE_UNIQUE_KEY_FIELD,
        audit_key_label: LIVE_UNIQUE_KEY_FIELD,
        sync_rule: activity_config.sync_rule,
        db_sync: activity_config.db_sync,
    }
}

/// 直播同步专属字段规则。
///
/// 表头来自直播数据源：
/// `直播间ID、开播时间、主播名称、主播uid、标题、直播曝光pv ...`
/// 这些字段规则固定在直播 pipeline 中，后续每期活动只替换表链接和表 ID。
fn live_field_rules() -> FieldValueRules {
    FieldValueRules::new()
        .with_skip_fields(&[
            "修改人",
            "创建人",
            "创建时间",
            "最后更新时间",
            "更新时间",
            "未添加稿件",
            "record_id",
            "recordurl",
            "sharedurl",
        ])
        .with_url_fields(&["链接(视频链接有效期为1小时)"])
        .with_text_fields(&[
            LIVE_UNIQUE_KEY_FIELD,
            "主播uid",
            "主播名称",
            "标题",
            "直播游戏名称",
            "数据来源",
        ])
        .with_number_fields(&[
            "累计观看人数",
            "评论量",
            "分享量",
            "组件点击数(延迟1天)",
            "获奖金额",
            "直播曝光pv",
            "曝光uv",
            "Acu",
            "Pcu",
            "直播时长",
            "人均观看时长",
            "涨粉数",
            "Android下载数/Android激活数",
            "ios下载数/ios激活数",
            "点赞率",
            "评论率",
        ])
        .with_timestamp_millis_fields(&["开播时间"])
}
