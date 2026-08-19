# 飞书电子表格分析文本格式化接口

## 接口

```http
POST /api/v1/admin/feishu/spreadsheets/format-analysis
Authorization: Bearer <MUTATION_API_TOKEN>
Content-Type: application/json
```

请求体：

```json
{
  "url": "https://example.feishu.cn/wiki/ExampleSpreadsheetToken?sheet=Sheet01",
  "dry_run": false
}
```

- `url`：必填。支持 `/sheets/{spreadsheet_token}?sheet={sheet_id}`、`/wiki/{spreadsheet_token}?sheet={sheet_id}`，也支持 Markdown 链接字符串。
- `dry_run`：可选，默认 `false`。设为 `true` 时只读取和统计，不修改电子表格。

调用示例：

```bash
curl -X POST 'https://api.example.com/api/v1/admin/feishu/spreadsheets/format-analysis' \
  -H 'Authorization: Bearer <MUTATION_API_TOKEN>' \
  -H 'Content-Type: application/json' \
  -d '{
    "url": "https://example.feishu.cn/wiki/ExampleSpreadsheetToken?sheet=Sheet01",
    "dry_run": true
  }'
```

## 处理规则

接口先用链接中的 `sheet_id` 调用飞书单范围读取接口，再逐个检查 `ROWS` 中的字符串单元格：

1. `数字）任意文字：` 整段加粗，例如 `1）实况解说类：`。
2. `↓数字%` 整段使用绿色 `#00D100`。
3. `↑数字%` 整段使用红色 `#D30000`。
4. 百分比数字严格大于 `50` 时，该百分比整段同时加粗；`50%` 不加粗，`50.1%` 加粗。
5. 布尔值、数字、未命中的字符串和空单元格不会回写。

命中内容会转换成飞书 Sheets 的文本片段数组，每个命中单元格单独作为一个范围写回。接口每批最多写 10 个范围，避免覆盖未命中的单元格。

批量写入时单个单元格也会使用飞书要求的闭区间，例如 `Sheet01!D1:D1`，不会再发送不被写入接口接受的 `Sheet01!D1`。

这里不会生成 `multipleValue`。`multipleValue` 是已经设置下拉列表的数据验证单元格使用的值类型；普通局部富文本使用 `type: "text"` 和 `segmentStyle`，字符串中可以正常保留中英文逗号。

## 响应

读取失败时，接口透传飞书的 HTTP 状态码和 `code/data/msg` 响应，不执行写入。

读取和写入成功时，响应保留飞书原始读取结果，并在 `data.formatting` 增加执行统计：

```json
{
  "code": 0,
  "data": {
    "revision": 18914,
    "spreadsheetToken": "ExampleSpreadsheetToken",
    "valueRange": {
      "majorDimension": "ROWS",
      "range": "Sheet01!A1:D2",
      "revision": 18914,
      "values": []
    },
    "formatting": {
      "sheetId": "Sheet01",
      "matchedCells": 1,
      "styledSegments": 6,
      "updatedCells": 1,
      "dryRun": false,
      "writeResponses": [
        {
          "code": 0,
          "msg": "success"
        }
      ]
    }
  },
  "msg": "success"
}
```

响应设置 `Cache-Control: no-store, private` 和 `Pragma: no-cache`，不应由浏览器或代理缓存。

如果分批写入期间飞书返回失败，接口透传该失败响应，并追加 `operationContext`，其中会说明已完成批次数、总批次数和命中单元格数。重复执行是安全的，已处理单元格只会被写入相同的富文本样式。
