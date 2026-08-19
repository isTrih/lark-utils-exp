# 审核扩展字段 `audit_extra`

`video_content` 和 `live_session` 都包含以下字段：

```sql
audit_extra JSONB NOT NULL DEFAULT '{}'::jsonb
```

该字段只保存 JSON 对象。审核结果同步读取飞书审核表时，会收集当前记录中所有以 `【额外】` 开头的字段，去掉前缀并清理键名首尾空白，然后与审核结果一起写入数据库。

例如审核表记录包含：

```json
{
  "审核结果": "审核通过",
  "【额外】塔塔二创": 1,
  "【额外】是否推荐": true,
  "【额外】驳回": false,
  "【额外】备注": "cxxxx"
}
```

数据库中的 `audit_extra` 为：

```json
{
  "塔塔二创": 1,
  "是否推荐": true,
  "驳回": false,
  "备注": "cxxxx"
}
```

## 类型规则

- JSON 数字、布尔值、字符串、数组和对象保持原类型。
- 飞书普通富文本值，例如 `[{"text":"cxxxx","type":"text"}]`，会转换为 JSON 字符串 `"cxxxx"`。
- 没有任何 `【额外】` 字段时保存 `{}`，用于清除数据库里已经失效的旧扩展字段。
- `【额外】` 后没有键名会终止本次审核回传并报告配置错误。
- 同一业务唯一键出现多条审核记录时，审核结果、审核标签和 `audit_extra` 必须完全一致，否则拒绝回传，避免不确定覆盖。

现有视频和直播明细查询接口会返回脱敏后的 `audit_extra`。需要按扩展字段精确筛选并取得最新指标和汇总时，使用下面的专用接口。

## 按扩展字段查询详情和汇总

```http
POST /api/v1/queries/v2/audit-extra/search
Content-Type: application/json
```

请求示例：

```json
{
  "project_id": 1,
  "activity_period_id": 2,
  "conditions": {
    "rok_key": "ROK",
    "key": "<保密值>"
  },
  "audit_result": "审核通过",
  "limit": 100,
  "offset": 0
}
```

- `project_id` 和 `activity_period_id` 都必填。接口会校验期次属于指定主项目，错配时返回 `404`。
- `conditions` 至少包含一个、最多包含 20 个键值；多个条件为 AND，键名和值均区分大小写。
- 值按 JSON 类型精确匹配：数字 `1`、字符串 `"1"`、布尔值 `true` 互不相等。
- 写入数据库前会移除飞书 `{ "type": 1, "value": [...] }` 等传输包装：富文本片段按顺序拼接成普通字符串，数字和布尔值保留原类型。新增 migration 也会清理已有包装值；查询期间仍兼容滚动发布时短暂出现的旧包装数据。
- 查询条件只填写业务值，例如 `{ "key": "<保密值>" }`，不要提交飞书富文本包装对象。
- JSON `null` 只匹配实际存在且值为 `null` 的键，不匹配缺少该键的记录。
- 对象必须完整相等；数组的元素和顺序必须完全相等，不使用 JSON 子集语义。
- `audit_result` 可省略；传入时精确匹配数据库审核结果，常见值为 `审核通过`、`不通过`。
- `limit` 和 `offset` 分别作用于视频列表和直播列表，汇总始终针对全部命中内容，不受分页影响。
- `audit_extra` 顶层字段名 `key` 是机密扩展项：它可以作为筛选条件，但绝不会出现在响应回显、视频详情、直播详情或其他数据接口中；含此条件的请求也不会写入查询缓存。嵌套对象内部的同名字段不属于这个顶层保密约定。

响应同时包含：

- `videos.data`：视频基础信息和最新一条 `video_daily_metric`；没有指标时 `latest_metric=null`。
- `live_sessions.data`：直播场次当前最新数据。
- `summary.total_play_count`：每个命中视频最新播放量的总和，不会累加历史快照。
- `summary.total_live_exposure_pv`：命中直播的 `live_exposure_pv` 总和，不使用累计观看人数替代。
- `summary.video_with_metric_count`：确实存在指标快照的视频数，便于识别“命中但尚无播放指标”的内容。

完整字段、错误响应和类型定义以 `/swagger-ui` 中的 `audit-extra` 分组为准。该接口是只读、幂等的 POST 查询；使用 POST 是为了完整保留条件值的 JSON 类型。
