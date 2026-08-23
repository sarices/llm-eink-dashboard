#!/bin/sh
# Output contract for a custom usage source. Configure this executable path in the app.
printf '%s\n' '{"schemaVersion":1,"source":"example","updatedAt":"2026-08-20T14:00:00Z","accounts":[{"id":"personal","label":"个人账户","balance":{"amount":12.5,"currency":"USD"},"models":[{"id":"example-model","period":"day","inputTokens":1200,"outputTokens":3400,"cachedTokens":0,"totalTokens":4600,"cost":{"amount":0.12,"currency":"USD"},"confidence":"exact"}]}]}'
