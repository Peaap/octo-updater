# OctoWoW launcher news feed

`news.json` is the public, static feed consumed by the desktop launcher at:

`https://peaap.github.io/octo-updater/news.json`

Keep the file valid JSON and use the following shape:

```json
{
  "featured": {
    "id": "unique-stable-id",
    "title": "Announcement title",
    "date": "2026-09-09T12:00:00Z",
    "author": "Author name",
    "body": "Plain-text announcement summary.",
    "url": "https://octowow.st/forum/viewtopic.php?t=123"
  },
  "patchNotes": [
    {
      "id": "unique-stable-id",
      "title": "Patch title",
      "date": "2026-09-09T12:00:00Z",
      "author": "Author name",
      "body": "Plain-text patch summary.",
      "url": "https://octowow.st/forum/viewtopic.php?t=456"
    }
  ]
}
```

The feed is intentionally public and must contain only information safe to publish. Use HTTPS links and plain text; do not include credentials, tokens, private player information, or executable download URLs.
