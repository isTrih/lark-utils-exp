use crate::client::LarkClient;
use crate::pipeline::activity::ActivityTableSyncConfig;
use crate::pipeline::sync::FieldValueRules;
use crate::pipeline::table_sync::{TableSyncConfig, run_table_sync_pipeline};

/// 视频同步业务固定的唯一键字段。
///
/// 每期活动会变的是 URL 和表 ID；唯一键属于视频同步业务规则，不由活动配置传入。
pub const VIDEO_UNIQUE_KEY_FIELD: &str = "视频/图文ID";

/// 运行视频同步 pipeline。
pub async fn run_video_sync(
    lark: LarkClient,
    activity_config: ActivityTableSyncConfig,
) -> anyhow::Result<()> {
    run_table_sync_pipeline(
        lark,
        build_table_sync_config(activity_config),
        video_field_rules(),
    )
    .await
}

/// 把每期活动可变配置转换成视频同步内部配置。
fn build_table_sync_config(activity_config: ActivityTableSyncConfig) -> TableSyncConfig {
    TableSyncConfig {
        pipeline_name: "video_sync",
        bitable_url: activity_config.bitable_url,
        spreadsheet_url: activity_config.spreadsheet_url,
        source_mode: activity_config.source_mode,
        manual_table_id: activity_config.manual_table_id,
        main_table_id: activity_config.main_table_id,
        audit_table_id: activity_config.audit_table_id,
        unique_key_field: VIDEO_UNIQUE_KEY_FIELD,
        audit_key_label: VIDEO_UNIQUE_KEY_FIELD,
        sync_rule: activity_config.sync_rule,
        db_sync: activity_config.db_sync,
    }
}

/// 视频同步专属字段规则。
///
/// 这里固定视频数据的字段转换语义：
/// - 只读字段/系统字段跳过
/// - URL、文本、数字、时间分别转换成多维表格可写格式
fn video_field_rules() -> FieldValueRules {
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
            VIDEO_UNIQUE_KEY_FIELD,
            "作者uid",
            "作者名称",
            "标题",
            "相关性审核",
            "投稿机构ID",
            "投稿机构名称",
            "数据来源",
        ])
        .with_number_fields(&[
            "播放量",
            "点赞量",
            "评论量",
            "分享量",
            "有效播放量",
            "有效点赞量",
            "组件点击数(延迟1天)",
            "获奖等级",
            "获奖金额",
            "Android激活数",
            "ios激活数",
            "预约成功次数(仅安卓)",
            "预约安装完成次数",
            "涨粉数",
            "点赞率",
            "评论率",
        ])
        .with_timestamp_millis_fields(&["发布时间"])
}
