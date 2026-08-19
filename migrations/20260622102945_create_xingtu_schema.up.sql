CREATE TYPE xingtu_content_type AS ENUM ('video', 'live');
COMMENT ON TYPE xingtu_content_type IS '星图数据类型：video=视频/图文，live=直播';

CREATE TYPE xingtu_receive_id_type AS ENUM ('open_id', 'union_id', 'user_id', 'email', 'chat_id');
COMMENT ON TYPE xingtu_receive_id_type IS '飞书消息接收者 ID 类型';

CREATE TYPE xingtu_spreadsheet_url_update_mode AS ENUM ('xingtu_export', 'manual');
COMMENT ON TYPE xingtu_spreadsheet_url_update_mode IS '源数据 Sheet 链接更新方式：xingtu_export=由星图导出流程更新，manual=人工维护';

CREATE TYPE xingtu_source_trigger_type AS ENUM ('morning', 'periodic', 'manual');
COMMENT ON TYPE xingtu_source_trigger_type IS '数据来源触发类型：morning=每日早上审核工作流，periodic=周期同步工作流，manual=人工手动拉取';

CREATE TYPE xingtu_import_status AS ENUM ('pending', 'imported', 'failed', 'partial');
COMMENT ON TYPE xingtu_import_status IS '导入状态：pending=待导入，imported=已导入，failed=导入失败，partial=部分导入';

CREATE TABLE xingtu_activity_period (
    activity_period_id BIGSERIAL PRIMARY KEY,
    project TEXT NOT NULL,
    period TEXT NOT NULL,
    period_code TEXT,
    task_month DATE NOT NULL,
    project_group_id TEXT NOT NULL,
    receive_id_type xingtu_receive_id_type NOT NULL DEFAULT 'chat_id',
    bitable_url TEXT NOT NULL,
    notice_card_template_id TEXT NOT NULL DEFAULT 'AAqNG59xifQWY',
    audit_result_field TEXT NOT NULL DEFAULT '审核结果',
    need_trace BOOLEAN NOT NULL DEFAULT true,
    morning_review_enabled BOOLEAN NOT NULL DEFAULT true,
    periodic_sync_enabled BOOLEAN NOT NULL DEFAULT true,
    periodic_sync_interval_hours INTEGER NOT NULL DEFAULT 2,
    tracking_start_date DATE,
    tracking_end_date DATE,
    is_active BOOLEAN NOT NULL DEFAULT true,
    remark TEXT,
    config_snapshot JSONB NOT NULL DEFAULT '{}'::jsonb,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (project, period),
    UNIQUE (period_code),
    CONSTRAINT chk_xingtu_activity_period_project_not_blank CHECK (btrim(project) <> ''),
    CONSTRAINT chk_xingtu_activity_period_period_not_blank CHECK (btrim(period) <> ''),
    CONSTRAINT chk_xingtu_activity_period_period_code_not_blank CHECK (period_code IS NULL OR btrim(period_code) <> ''),
    CONSTRAINT chk_xingtu_activity_period_group_id_not_blank CHECK (btrim(project_group_id) <> ''),
    CONSTRAINT chk_xingtu_activity_period_bitable_url_not_blank CHECK (btrim(bitable_url) <> ''),
    CONSTRAINT chk_xingtu_activity_period_card_template_not_blank CHECK (btrim(notice_card_template_id) <> ''),
    CONSTRAINT chk_xingtu_activity_period_audit_result_field_not_blank CHECK (btrim(audit_result_field) <> ''),
    CONSTRAINT chk_xingtu_activity_period_periodic_interval CHECK (periodic_sync_interval_hours > 0),
    CONSTRAINT chk_xingtu_activity_period_month_first_day CHECK (task_month = date_trunc('month', task_month)::date),
    CONSTRAINT chk_xingtu_activity_period_tracking_date CHECK (
        tracking_start_date IS NULL
        OR tracking_end_date IS NULL
        OR tracking_end_date >= tracking_start_date
    )
);
COMMENT ON TABLE xingtu_activity_period IS '星图活动期次配置表，一期活动一行，承载通知群、卡片模板、追踪周期等公共配置';
COMMENT ON COLUMN xingtu_activity_period.activity_period_id IS '活动期次内部 ID';
COMMENT ON COLUMN xingtu_activity_period.project IS '项目名，例如 ROK';
COMMENT ON COLUMN xingtu_activity_period.period IS '活动期次展示名，例如 2026年7月第十四期';
COMMENT ON COLUMN xingtu_activity_period.period_code IS '活动期次稳定编码，例如 rok-2026-07-p14，便于后端任务配置引用';
COMMENT ON COLUMN xingtu_activity_period.task_month IS '任务所属月份，必须存每月 1 号';
COMMENT ON COLUMN xingtu_activity_period.project_group_id IS '项目通知群或接收者 ID';
COMMENT ON COLUMN xingtu_activity_period.receive_id_type IS '项目通知接收者 ID 类型，默认 chat_id';
COMMENT ON COLUMN xingtu_activity_period.bitable_url IS '本期唯一多维表格链接；通知卡片中的 audit_table_url 直接使用这个链接';
COMMENT ON COLUMN xingtu_activity_period.notice_card_template_id IS '审核通知卡片模板 ID';
COMMENT ON COLUMN xingtu_activity_period.audit_result_field IS '审核表中判断待审核状态的字段名';
COMMENT ON COLUMN xingtu_activity_period.need_trace IS '是否需要定时追踪拉取星图数据';
COMMENT ON COLUMN xingtu_activity_period.morning_review_enabled IS '是否启用每日早上审核工作流：拉取、同步、发送审核通知';
COMMENT ON COLUMN xingtu_activity_period.periodic_sync_enabled IS '是否启用周期同步工作流：拉取并同步，不发送审核通知';
COMMENT ON COLUMN xingtu_activity_period.periodic_sync_interval_hours IS '周期同步间隔小时数，调度器可按此生成定时任务';
COMMENT ON COLUMN xingtu_activity_period.tracking_start_date IS '追踪开始日期';
COMMENT ON COLUMN xingtu_activity_period.tracking_end_date IS '追踪结束日期';
COMMENT ON COLUMN xingtu_activity_period.is_active IS '配置是否启用';
COMMENT ON COLUMN xingtu_activity_period.remark IS '备注';
COMMENT ON COLUMN xingtu_activity_period.config_snapshot IS '原始配置快照，方便调试和排查配置导入问题';
COMMENT ON COLUMN xingtu_activity_period.created_at IS '创建时间';
COMMENT ON COLUMN xingtu_activity_period.updated_at IS '更新时间';
CREATE INDEX idx_xingtu_activity_period_project_month ON xingtu_activity_period (project, task_month);
CREATE INDEX idx_xingtu_activity_period_active ON xingtu_activity_period (is_active);
CREATE INDEX idx_xingtu_activity_period_tracking ON xingtu_activity_period (tracking_start_date, tracking_end_date) WHERE need_trace = true;
CREATE INDEX idx_xingtu_activity_period_morning_workflow ON xingtu_activity_period (is_active, task_month) WHERE morning_review_enabled = true;
CREATE INDEX idx_xingtu_activity_period_periodic_workflow ON xingtu_activity_period (is_active, tracking_start_date, tracking_end_date) WHERE periodic_sync_enabled = true;

CREATE TABLE xingtu_activity_content_config (
    content_config_id BIGSERIAL PRIMARY KEY,
    activity_period_id BIGINT NOT NULL REFERENCES xingtu_activity_period(activity_period_id) ON DELETE CASCADE,
    content_type xingtu_content_type NOT NULL,
    xingtu_task_id TEXT NOT NULL,
    xingtu_task_name TEXT,
    source_spreadsheet_url TEXT,
    source_spreadsheet_url_update_mode xingtu_spreadsheet_url_update_mode NOT NULL DEFAULT 'xingtu_export',
    source_spreadsheet_url_updated_at TIMESTAMPTZ,
    manual_table_id TEXT,
    main_table_id TEXT NOT NULL,
    audit_table_id TEXT NOT NULL,
    data_source_field TEXT NOT NULL DEFAULT '数据来源',
    spreadsheet_source_value TEXT NOT NULL DEFAULT '星图数据',
    manual_source_value TEXT NOT NULL DEFAULT '手动登记',
    manual_overrides_spreadsheet BOOLEAN NOT NULL DEFAULT true,
    manual_auto_approve BOOLEAN NOT NULL DEFAULT true,
    manual_auto_approve_result TEXT NOT NULL DEFAULT '审核通过',
    sync_enabled BOOLEAN NOT NULL DEFAULT true,
    trace_enabled BOOLEAN NOT NULL DEFAULT true,
    remark TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (activity_period_id, content_type),
    UNIQUE (xingtu_task_id),
    UNIQUE (content_config_id, content_type),
    CONSTRAINT chk_xingtu_activity_content_task_id_not_blank CHECK (btrim(xingtu_task_id) <> ''),
    CONSTRAINT chk_xingtu_activity_content_source_url_not_blank CHECK (source_spreadsheet_url IS NULL OR btrim(source_spreadsheet_url) <> ''),
    CONSTRAINT chk_xingtu_activity_content_manual_id_not_blank CHECK (manual_table_id IS NULL OR btrim(manual_table_id) <> ''),
    CONSTRAINT chk_xingtu_activity_content_main_id_not_blank CHECK (btrim(main_table_id) <> ''),
    CONSTRAINT chk_xingtu_activity_content_audit_id_not_blank CHECK (btrim(audit_table_id) <> ''),
    CONSTRAINT chk_xingtu_activity_content_data_source_field_not_blank CHECK (btrim(data_source_field) <> ''),
    CONSTRAINT chk_xingtu_activity_content_spreadsheet_source_not_blank CHECK (btrim(spreadsheet_source_value) <> ''),
    CONSTRAINT chk_xingtu_activity_content_manual_source_not_blank CHECK (btrim(manual_source_value) <> ''),
    CONSTRAINT chk_xingtu_activity_content_auto_result_not_blank CHECK (btrim(manual_auto_approve_result) <> '')
);
COMMENT ON TABLE xingtu_activity_content_config IS '星图活动内容同步配置表，一期活动的直播/视频各一行';
COMMENT ON COLUMN xingtu_activity_content_config.content_config_id IS '内容同步配置 ID';
COMMENT ON COLUMN xingtu_activity_content_config.activity_period_id IS '所属活动期次 ID';
COMMENT ON COLUMN xingtu_activity_content_config.content_type IS '内容类型：live=直播，video=视频/图文';
COMMENT ON COLUMN xingtu_activity_content_config.xingtu_task_id IS '星图任务 ID，外部平台全局唯一 ID';
COMMENT ON COLUMN xingtu_activity_content_config.xingtu_task_name IS '星图任务名称';
COMMENT ON COLUMN xingtu_activity_content_config.source_spreadsheet_url IS '当前星图导出的源数据 Sheet 链接，可由拉取流程更新';
COMMENT ON COLUMN xingtu_activity_content_config.source_spreadsheet_url_update_mode IS '源数据 Sheet 链接更新方式';
COMMENT ON COLUMN xingtu_activity_content_config.source_spreadsheet_url_updated_at IS '源数据 Sheet 链接更新时间';
COMMENT ON COLUMN xingtu_activity_content_config.manual_table_id IS '手动登记表 ID，没有手动登记时为空';
COMMENT ON COLUMN xingtu_activity_content_config.main_table_id IS '业务主表 ID';
COMMENT ON COLUMN xingtu_activity_content_config.audit_table_id IS '审核表 ID';
COMMENT ON COLUMN xingtu_activity_content_config.data_source_field IS '主表中标记数据来源的字段名';
COMMENT ON COLUMN xingtu_activity_content_config.spreadsheet_source_value IS '星图源数据写入数据来源字段的值';
COMMENT ON COLUMN xingtu_activity_content_config.manual_source_value IS '手动登记数据写入数据来源字段的值';
COMMENT ON COLUMN xingtu_activity_content_config.manual_overrides_spreadsheet IS '同一唯一键同时存在星图和手动登记数据时，是否手动登记覆盖星图';
COMMENT ON COLUMN xingtu_activity_content_config.manual_auto_approve IS '手动登记数据同步 audit 表时是否自动审核通过';
COMMENT ON COLUMN xingtu_activity_content_config.manual_auto_approve_result IS '手动登记数据自动审核通过时写入审核结果字段的值';
COMMENT ON COLUMN xingtu_activity_content_config.sync_enabled IS '是否启用同步';
COMMENT ON COLUMN xingtu_activity_content_config.trace_enabled IS '是否启用星图定时拉取';
COMMENT ON COLUMN xingtu_activity_content_config.remark IS '备注';
COMMENT ON COLUMN xingtu_activity_content_config.created_at IS '创建时间';
COMMENT ON COLUMN xingtu_activity_content_config.updated_at IS '更新时间';
CREATE INDEX idx_xingtu_activity_content_period ON xingtu_activity_content_config (activity_period_id);
CREATE INDEX idx_xingtu_activity_content_type ON xingtu_activity_content_config (content_type);
CREATE INDEX idx_xingtu_activity_content_sync_enabled ON xingtu_activity_content_config (sync_enabled);
CREATE INDEX idx_xingtu_activity_content_trace_enabled ON xingtu_activity_content_config (trace_enabled);

CREATE TABLE xingtu_activity_auditor (
    activity_auditor_id BIGSERIAL PRIMARY KEY,
    activity_period_id BIGINT NOT NULL REFERENCES xingtu_activity_period(activity_period_id) ON DELETE CASCADE,
    auditor_name TEXT NOT NULL,
    auditor_id TEXT NOT NULL,
    auditor_id_type xingtu_receive_id_type NOT NULL DEFAULT 'user_id',
    sort_order INTEGER NOT NULL DEFAULT 0,
    is_active BOOLEAN NOT NULL DEFAULT true,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (activity_period_id, auditor_id),
    CONSTRAINT chk_xingtu_activity_auditor_name_not_blank CHECK (btrim(auditor_name) <> ''),
    CONSTRAINT chk_xingtu_activity_auditor_id_not_blank CHECK (btrim(auditor_id) <> '')
);
COMMENT ON TABLE xingtu_activity_auditor IS '星图活动审核人配置表，用于生成卡片模板变量 auditor_ids';
COMMENT ON COLUMN xingtu_activity_auditor.activity_auditor_id IS '审核人配置 ID';
COMMENT ON COLUMN xingtu_activity_auditor.activity_period_id IS '所属活动期次 ID';
COMMENT ON COLUMN xingtu_activity_auditor.auditor_name IS '审核人姓名';
COMMENT ON COLUMN xingtu_activity_auditor.auditor_id IS '审核人在飞书中的 ID';
COMMENT ON COLUMN xingtu_activity_auditor.auditor_id_type IS '审核人 ID 类型';
COMMENT ON COLUMN xingtu_activity_auditor.sort_order IS '排序值，用于稳定生成 auditor_ids';
COMMENT ON COLUMN xingtu_activity_auditor.is_active IS '是否启用该审核人';
COMMENT ON COLUMN xingtu_activity_auditor.created_at IS '创建时间';
COMMENT ON COLUMN xingtu_activity_auditor.updated_at IS '更新时间';
CREATE INDEX idx_xingtu_activity_auditor_period_active ON xingtu_activity_auditor (activity_period_id, is_active, sort_order);

CREATE TABLE xingtu_feishu_source (
    feishu_source_id BIGSERIAL PRIMARY KEY,
    content_config_id BIGINT NOT NULL REFERENCES xingtu_activity_content_config(content_config_id),
    content_type xingtu_content_type NOT NULL,
    feishu_sheet_url TEXT NOT NULL,
    trigger_type xingtu_source_trigger_type NOT NULL,
    stat_date DATE NOT NULL,
    pulled_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    is_daily_final BOOLEAN NOT NULL DEFAULT false,
    is_imported BOOLEAN NOT NULL DEFAULT false,
    import_status xingtu_import_status NOT NULL DEFAULT 'pending',
    imported_row_count INTEGER NOT NULL DEFAULT 0,
    imported_at TIMESTAMPTZ,
    error_message TEXT,
    created_by TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (feishu_sheet_url),
    FOREIGN KEY (content_config_id, content_type) REFERENCES xingtu_activity_content_config(content_config_id, content_type),
    CONSTRAINT chk_xingtu_feishu_source_imported_row_count CHECK (imported_row_count >= 0)
);
COMMENT ON TABLE xingtu_feishu_source IS '飞书数据来源表，每一次拉取对应一个飞书表格链接，程序读取该链接后导入数据库';
COMMENT ON COLUMN xingtu_feishu_source.feishu_source_id IS '飞书数据来源 ID';
COMMENT ON COLUMN xingtu_feishu_source.content_config_id IS '内容同步配置 ID，关联直播或视频配置';
COMMENT ON COLUMN xingtu_feishu_source.content_type IS '类型：直播，视频';
COMMENT ON COLUMN xingtu_feishu_source.feishu_sheet_url IS '飞书表格链接';
COMMENT ON COLUMN xingtu_feishu_source.trigger_type IS '数据来源触发类型';
COMMENT ON COLUMN xingtu_feishu_source.stat_date IS '数据归属日期';
COMMENT ON COLUMN xingtu_feishu_source.pulled_at IS '实际拉取时间';
COMMENT ON COLUMN xingtu_feishu_source.is_daily_final IS '是否为每日最终数据';
COMMENT ON COLUMN xingtu_feishu_source.is_imported IS '是否导入';
COMMENT ON COLUMN xingtu_feishu_source.import_status IS '导入状态';
COMMENT ON COLUMN xingtu_feishu_source.imported_row_count IS '导入行数';
COMMENT ON COLUMN xingtu_feishu_source.imported_at IS '导入完成时间';
COMMENT ON COLUMN xingtu_feishu_source.error_message IS '错误信息';
COMMENT ON COLUMN xingtu_feishu_source.created_by IS '创建人或触发来源，例如 system/manual';
COMMENT ON COLUMN xingtu_feishu_source.created_at IS '创建时间';
COMMENT ON COLUMN xingtu_feishu_source.updated_at IS '更新时间';
CREATE INDEX idx_feishu_source_config_date ON xingtu_feishu_source (content_config_id, stat_date);
CREATE INDEX idx_feishu_source_type_date ON xingtu_feishu_source (content_type, stat_date);
CREATE INDEX idx_feishu_source_import_status ON xingtu_feishu_source (import_status);
CREATE INDEX idx_feishu_source_pulled_at ON xingtu_feishu_source (pulled_at);
CREATE UNIQUE INDEX uq_feishu_source_daily_final ON xingtu_feishu_source (content_config_id, stat_date) WHERE is_daily_final = true;

CREATE TABLE video_content (
    video_id TEXT NOT NULL,
    content_config_id BIGINT NOT NULL REFERENCES xingtu_activity_content_config(content_config_id),
    first_feishu_source_id BIGINT REFERENCES xingtu_feishu_source(feishu_source_id),
    publish_time TIMESTAMP NOT NULL,
    author_name TEXT,
    author_uid TEXT,
    title TEXT,
    relevance_review TEXT,
    award_level INTEGER,
    award_amount NUMERIC(18, 2),
    submit_org_id TEXT,
    submit_org_name TEXT,
    latest_video_url TEXT,
    latest_video_url_updated_at TIMESTAMPTZ,
    first_seen_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_seen_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (content_config_id, video_id),
    CONSTRAINT chk_video_content_award_amount CHECK (award_amount IS NULL OR award_amount >= 0)
);
COMMENT ON TABLE video_content IS '视频/图文基础信息表，一个内容配置+一个视频/图文 ID 一行';
COMMENT ON COLUMN video_content.video_id IS '视频/图文 ID，外部平台 ID，使用 TEXT 避免超出 BIGINT 范围';
COMMENT ON COLUMN video_content.content_config_id IS '内容同步配置 ID';
COMMENT ON COLUMN video_content.first_feishu_source_id IS '首次导入该视频/图文的飞书数据来源 ID';
COMMENT ON COLUMN video_content.publish_time IS '发布时间';
COMMENT ON COLUMN video_content.author_name IS '作者名称';
COMMENT ON COLUMN video_content.author_uid IS '作者 uid，外部平台 ID，使用 TEXT 避免超出 BIGINT 范围';
COMMENT ON COLUMN video_content.title IS '标题';
COMMENT ON COLUMN video_content.relevance_review IS '相关性审核';
COMMENT ON COLUMN video_content.award_level IS '获奖等级';
COMMENT ON COLUMN video_content.award_amount IS '获奖金额';
COMMENT ON COLUMN video_content.submit_org_id IS '投稿机构 ID，外部平台 ID，使用 TEXT 避免超出 BIGINT 范围';
COMMENT ON COLUMN video_content.submit_org_name IS '投稿机构名称';
COMMENT ON COLUMN video_content.latest_video_url IS '链接(视频链接有效期为1小时)';
COMMENT ON COLUMN video_content.latest_video_url_updated_at IS '链接更新时间';
COMMENT ON COLUMN video_content.first_seen_at IS '首次采集到该视频/图文的时间';
COMMENT ON COLUMN video_content.last_seen_at IS '最近一次采集到该视频/图文的时间';
COMMENT ON COLUMN video_content.created_at IS '创建时间';
COMMENT ON COLUMN video_content.updated_at IS '更新时间';
CREATE INDEX idx_video_content_config_id ON video_content (content_config_id);
CREATE INDEX idx_video_content_publish_time ON video_content (publish_time);
CREATE INDEX idx_video_content_author_uid ON video_content (author_uid);
CREATE INDEX idx_video_content_submit_org_id ON video_content (submit_org_id);
CREATE INDEX idx_video_content_first_feishu_source ON video_content (first_feishu_source_id);

CREATE TABLE video_daily_metric (
    content_config_id BIGINT NOT NULL REFERENCES xingtu_activity_content_config(content_config_id),
    feishu_source_id BIGINT NOT NULL REFERENCES xingtu_feishu_source(feishu_source_id),
    stat_date DATE NOT NULL,
    video_id TEXT NOT NULL,
    play_count BIGINT,
    valid_play_count BIGINT,
    like_count BIGINT,
    valid_like_count BIGINT,
    comment_count BIGINT,
    share_count BIGINT,
    component_click_count BIGINT,
    android_activate_count BIGINT,
    ios_activate_count BIGINT,
    reservation_success_count BIGINT,
    reservation_install_complete_count BIGINT,
    follower_increase_count BIGINT,
    like_rate NUMERIC(12, 6),
    comment_rate NUMERIC(12, 6),
    is_daily_final BOOLEAN NOT NULL DEFAULT false,
    pulled_at TIMESTAMPTZ,
    imported_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (content_config_id, stat_date, video_id),
    FOREIGN KEY (content_config_id, video_id) REFERENCES video_content(content_config_id, video_id),
    CONSTRAINT chk_video_daily_metric_non_negative_counts CHECK (
        (play_count IS NULL OR play_count >= 0)
        AND (valid_play_count IS NULL OR valid_play_count >= 0)
        AND (like_count IS NULL OR like_count >= 0)
        AND (valid_like_count IS NULL OR valid_like_count >= 0)
        AND (comment_count IS NULL OR comment_count >= 0)
        AND (share_count IS NULL OR share_count >= 0)
        AND (component_click_count IS NULL OR component_click_count >= 0)
        AND (android_activate_count IS NULL OR android_activate_count >= 0)
        AND (ios_activate_count IS NULL OR ios_activate_count >= 0)
        AND (reservation_success_count IS NULL OR reservation_success_count >= 0)
        AND (reservation_install_complete_count IS NULL OR reservation_install_complete_count >= 0)
        AND (follower_increase_count IS NULL OR follower_increase_count >= 0)
    ),
    CONSTRAINT chk_video_daily_metric_rates CHECK (
        (like_rate IS NULL OR like_rate >= 0)
        AND (comment_rate IS NULL OR comment_rate >= 0)
    )
) PARTITION BY RANGE (stat_date);
COMMENT ON TABLE video_daily_metric IS '视频/图文每日指标表，一个内容配置+一个视频/图文 ID+一天一行，只保存每日变化指标';
COMMENT ON COLUMN video_daily_metric.content_config_id IS '内容同步配置 ID';
COMMENT ON COLUMN video_daily_metric.feishu_source_id IS '飞书数据来源 ID';
COMMENT ON COLUMN video_daily_metric.stat_date IS '数据日期';
COMMENT ON COLUMN video_daily_metric.video_id IS '视频/图文 ID，外部平台 ID，使用 TEXT 避免超出 BIGINT 范围';
COMMENT ON COLUMN video_daily_metric.play_count IS '播放量';
COMMENT ON COLUMN video_daily_metric.valid_play_count IS '有效播放量';
COMMENT ON COLUMN video_daily_metric.like_count IS '点赞量';
COMMENT ON COLUMN video_daily_metric.valid_like_count IS '有效点赞量';
COMMENT ON COLUMN video_daily_metric.comment_count IS '评论量';
COMMENT ON COLUMN video_daily_metric.share_count IS '分享量';
COMMENT ON COLUMN video_daily_metric.component_click_count IS '组件点击数(延迟1天)';
COMMENT ON COLUMN video_daily_metric.android_activate_count IS 'Android激活数';
COMMENT ON COLUMN video_daily_metric.ios_activate_count IS 'ios激活数';
COMMENT ON COLUMN video_daily_metric.reservation_success_count IS '预约成功次数(仅安卓)';
COMMENT ON COLUMN video_daily_metric.reservation_install_complete_count IS '预约安装完成次数';
COMMENT ON COLUMN video_daily_metric.follower_increase_count IS '涨粉数';
COMMENT ON COLUMN video_daily_metric.like_rate IS '点赞率';
COMMENT ON COLUMN video_daily_metric.comment_rate IS '评论率';
COMMENT ON COLUMN video_daily_metric.is_daily_final IS '是否为每日最终数据';
COMMENT ON COLUMN video_daily_metric.pulled_at IS '实际拉取时间';
COMMENT ON COLUMN video_daily_metric.imported_at IS '导入时间';
COMMENT ON COLUMN video_daily_metric.updated_at IS '更新时间';

CREATE TABLE video_daily_metric_import_history (
    feishu_source_id BIGINT NOT NULL REFERENCES xingtu_feishu_source(feishu_source_id),
    content_config_id BIGINT NOT NULL REFERENCES xingtu_activity_content_config(content_config_id),
    stat_date DATE NOT NULL,
    video_id TEXT NOT NULL,
    play_count BIGINT,
    valid_play_count BIGINT,
    like_count BIGINT,
    valid_like_count BIGINT,
    comment_count BIGINT,
    share_count BIGINT,
    component_click_count BIGINT,
    android_activate_count BIGINT,
    ios_activate_count BIGINT,
    reservation_success_count BIGINT,
    reservation_install_complete_count BIGINT,
    follower_increase_count BIGINT,
    like_rate NUMERIC(12, 6),
    comment_rate NUMERIC(12, 6),
    imported_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (feishu_source_id, video_id),
    FOREIGN KEY (content_config_id, video_id) REFERENCES video_content(content_config_id, video_id),
    CONSTRAINT chk_video_metric_history_non_negative_counts CHECK (
        (play_count IS NULL OR play_count >= 0)
        AND (valid_play_count IS NULL OR valid_play_count >= 0)
        AND (like_count IS NULL OR like_count >= 0)
        AND (valid_like_count IS NULL OR valid_like_count >= 0)
        AND (comment_count IS NULL OR comment_count >= 0)
        AND (share_count IS NULL OR share_count >= 0)
        AND (component_click_count IS NULL OR component_click_count >= 0)
        AND (android_activate_count IS NULL OR android_activate_count >= 0)
        AND (ios_activate_count IS NULL OR ios_activate_count >= 0)
        AND (reservation_success_count IS NULL OR reservation_success_count >= 0)
        AND (reservation_install_complete_count IS NULL OR reservation_install_complete_count >= 0)
        AND (follower_increase_count IS NULL OR follower_increase_count >= 0)
    ),
    CONSTRAINT chk_video_metric_history_rates CHECK (
        (like_rate IS NULL OR like_rate >= 0)
        AND (comment_rate IS NULL OR comment_rate >= 0)
    )
);
COMMENT ON TABLE video_daily_metric_import_history IS '视频/图文每日指标导入历史表，保留每一次飞书来源导入的原始指标数据';
COMMENT ON COLUMN video_daily_metric_import_history.feishu_source_id IS '飞书数据来源 ID';
COMMENT ON COLUMN video_daily_metric_import_history.content_config_id IS '内容同步配置 ID';
COMMENT ON COLUMN video_daily_metric_import_history.stat_date IS '数据日期';
COMMENT ON COLUMN video_daily_metric_import_history.video_id IS '视频/图文 ID，外部平台 ID，使用 TEXT 避免超出 BIGINT 范围';
COMMENT ON COLUMN video_daily_metric_import_history.play_count IS '播放量';
COMMENT ON COLUMN video_daily_metric_import_history.valid_play_count IS '有效播放量';
COMMENT ON COLUMN video_daily_metric_import_history.like_count IS '点赞量';
COMMENT ON COLUMN video_daily_metric_import_history.valid_like_count IS '有效点赞量';
COMMENT ON COLUMN video_daily_metric_import_history.comment_count IS '评论量';
COMMENT ON COLUMN video_daily_metric_import_history.share_count IS '分享量';
COMMENT ON COLUMN video_daily_metric_import_history.component_click_count IS '组件点击数(延迟1天)';
COMMENT ON COLUMN video_daily_metric_import_history.android_activate_count IS 'Android激活数';
COMMENT ON COLUMN video_daily_metric_import_history.ios_activate_count IS 'ios激活数';
COMMENT ON COLUMN video_daily_metric_import_history.reservation_success_count IS '预约成功次数(仅安卓)';
COMMENT ON COLUMN video_daily_metric_import_history.reservation_install_complete_count IS '预约安装完成次数';
COMMENT ON COLUMN video_daily_metric_import_history.follower_increase_count IS '涨粉数';
COMMENT ON COLUMN video_daily_metric_import_history.like_rate IS '点赞率';
COMMENT ON COLUMN video_daily_metric_import_history.comment_rate IS '评论率';
COMMENT ON COLUMN video_daily_metric_import_history.imported_at IS '导入时间';
CREATE INDEX idx_video_metric_history_config_date ON video_daily_metric_import_history (content_config_id, stat_date);
CREATE INDEX idx_video_metric_history_video_date ON video_daily_metric_import_history (video_id, stat_date);
CREATE INDEX idx_video_metric_history_feishu_source ON video_daily_metric_import_history (feishu_source_id);

CREATE TABLE live_session (
    content_config_id BIGINT NOT NULL REFERENCES xingtu_activity_content_config(content_config_id),
    feishu_source_id BIGINT NOT NULL REFERENCES xingtu_feishu_source(feishu_source_id),
    live_room_id TEXT NOT NULL,
    start_time TIMESTAMP NOT NULL,
    anchor_name TEXT,
    anchor_uid TEXT,
    title TEXT,
    cumulative_viewer_count BIGINT,
    comment_count BIGINT,
    share_count BIGINT,
    component_click_count BIGINT,
    android_download_or_activate_count BIGINT,
    ios_download_or_activate_count BIGINT,
    live_url TEXT,
    live_url_updated_at TIMESTAMPTZ,
    award_amount NUMERIC(18, 2),
    live_exposure_pv BIGINT,
    exposure_uv BIGINT,
    acu NUMERIC(18, 4),
    pcu BIGINT,
    live_duration_seconds BIGINT,
    avg_watch_duration_seconds BIGINT,
    follower_increase_count BIGINT,
    like_rate NUMERIC(12, 6),
    comment_rate NUMERIC(12, 6),
    live_game_name TEXT,
    pulled_at TIMESTAMPTZ,
    first_seen_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_seen_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (content_config_id, live_room_id),
    CONSTRAINT chk_live_session_award_amount CHECK (award_amount IS NULL OR award_amount >= 0),
    CONSTRAINT chk_live_session_non_negative_counts CHECK (
        (cumulative_viewer_count IS NULL OR cumulative_viewer_count >= 0)
        AND (comment_count IS NULL OR comment_count >= 0)
        AND (share_count IS NULL OR share_count >= 0)
        AND (component_click_count IS NULL OR component_click_count >= 0)
        AND (android_download_or_activate_count IS NULL OR android_download_or_activate_count >= 0)
        AND (ios_download_or_activate_count IS NULL OR ios_download_or_activate_count >= 0)
        AND (live_exposure_pv IS NULL OR live_exposure_pv >= 0)
        AND (exposure_uv IS NULL OR exposure_uv >= 0)
        AND (pcu IS NULL OR pcu >= 0)
        AND (live_duration_seconds IS NULL OR live_duration_seconds >= 0)
        AND (avg_watch_duration_seconds IS NULL OR avg_watch_duration_seconds >= 0)
        AND (follower_increase_count IS NULL OR follower_increase_count >= 0)
    ),
    CONSTRAINT chk_live_session_rates CHECK (
        (like_rate IS NULL OR like_rate >= 0)
        AND (comment_rate IS NULL OR comment_rate >= 0)
    )
);
COMMENT ON TABLE live_session IS '直播场次表，一个内容配置+一个直播间 ID 一行，保存最新直播数据';
COMMENT ON COLUMN live_session.content_config_id IS '内容同步配置 ID';
COMMENT ON COLUMN live_session.feishu_source_id IS '飞书数据来源 ID';
COMMENT ON COLUMN live_session.live_room_id IS '直播间 ID，外部平台 ID，使用 TEXT 避免超出 BIGINT 范围';
COMMENT ON COLUMN live_session.start_time IS '开播时间';
COMMENT ON COLUMN live_session.anchor_name IS '主播名称';
COMMENT ON COLUMN live_session.anchor_uid IS '主播 uid，外部平台 ID，使用 TEXT 避免超出 BIGINT 范围';
COMMENT ON COLUMN live_session.title IS '标题';
COMMENT ON COLUMN live_session.cumulative_viewer_count IS '累计观看人数';
COMMENT ON COLUMN live_session.comment_count IS '评论量';
COMMENT ON COLUMN live_session.share_count IS '分享量';
COMMENT ON COLUMN live_session.component_click_count IS '组件点击数(延迟1天)';
COMMENT ON COLUMN live_session.android_download_or_activate_count IS 'Android下载数/Android激活数';
COMMENT ON COLUMN live_session.ios_download_or_activate_count IS 'ios下载数/ios激活数';
COMMENT ON COLUMN live_session.live_url IS '链接(视频链接有效期为1小时)';
COMMENT ON COLUMN live_session.live_url_updated_at IS '链接更新时间';
COMMENT ON COLUMN live_session.award_amount IS '获奖金额';
COMMENT ON COLUMN live_session.live_exposure_pv IS '直播曝光 pv';
COMMENT ON COLUMN live_session.exposure_uv IS '曝光 uv';
COMMENT ON COLUMN live_session.acu IS 'Acu';
COMMENT ON COLUMN live_session.pcu IS 'Pcu';
COMMENT ON COLUMN live_session.live_duration_seconds IS '直播时长，单位秒';
COMMENT ON COLUMN live_session.avg_watch_duration_seconds IS '人均观看时长，单位秒';
COMMENT ON COLUMN live_session.follower_increase_count IS '涨粉数';
COMMENT ON COLUMN live_session.like_rate IS '点赞率';
COMMENT ON COLUMN live_session.comment_rate IS '评论率';
COMMENT ON COLUMN live_session.live_game_name IS '直播游戏名称';
COMMENT ON COLUMN live_session.pulled_at IS '实际拉取时间';
COMMENT ON COLUMN live_session.first_seen_at IS '首次采集到该直播场次的时间';
COMMENT ON COLUMN live_session.last_seen_at IS '最近一次采集到该直播场次的时间';
COMMENT ON COLUMN live_session.created_at IS '创建时间';
COMMENT ON COLUMN live_session.updated_at IS '更新时间';
CREATE INDEX idx_live_session_config_id ON live_session (content_config_id);
CREATE INDEX idx_live_session_feishu_source ON live_session (feishu_source_id);
CREATE INDEX idx_live_session_start_time ON live_session (start_time);
CREATE INDEX idx_live_session_anchor_uid ON live_session (anchor_uid);
CREATE INDEX idx_live_session_game_name ON live_session (live_game_name);
CREATE INDEX idx_live_session_config_start_time ON live_session (content_config_id, start_time);

CREATE INDEX idx_video_daily_metric_config_date ON video_daily_metric (content_config_id, stat_date);
CREATE INDEX idx_video_daily_metric_video_date ON video_daily_metric (video_id, stat_date);
CREATE INDEX idx_video_daily_metric_config_video_date ON video_daily_metric (content_config_id, video_id, stat_date);
CREATE INDEX idx_video_daily_metric_feishu_source ON video_daily_metric (feishu_source_id);
CREATE INDEX idx_video_daily_metric_final ON video_daily_metric (content_config_id, stat_date, is_daily_final);

CREATE OR REPLACE FUNCTION set_updated_at()
RETURNS TRIGGER AS $$
BEGIN
    NEW.updated_at = now();
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER trg_xingtu_activity_period_updated_at BEFORE UPDATE ON xingtu_activity_period FOR EACH ROW EXECUTE FUNCTION set_updated_at();
CREATE TRIGGER trg_xingtu_activity_content_config_updated_at BEFORE UPDATE ON xingtu_activity_content_config FOR EACH ROW EXECUTE FUNCTION set_updated_at();
CREATE TRIGGER trg_xingtu_activity_auditor_updated_at BEFORE UPDATE ON xingtu_activity_auditor FOR EACH ROW EXECUTE FUNCTION set_updated_at();
CREATE TRIGGER trg_xingtu_feishu_source_updated_at BEFORE UPDATE ON xingtu_feishu_source FOR EACH ROW EXECUTE FUNCTION set_updated_at();
CREATE TRIGGER trg_video_content_updated_at BEFORE UPDATE ON video_content FOR EACH ROW EXECUTE FUNCTION set_updated_at();
CREATE TRIGGER trg_live_session_updated_at BEFORE UPDATE ON live_session FOR EACH ROW EXECUTE FUNCTION set_updated_at();
