import { test } from 'node:test';
import assert from 'node:assert/strict';
import { SettlersClient } from '../src/index.ts';
test('prepared commands retain one UUID across ambiguous retries', () => {
  const client = new SettlersClient('http://127.0.0.1:8080', 'saved-token');
  const request = client.prepare(8, { type: 'act', action: { type: 'roll' } });
  assert.equal(request.expected_version, 8);
  assert.match(request.request_id, /^[a-f0-9-]{36}$/);
  assert.deepEqual(JSON.parse(JSON.stringify(request)), request);
});
