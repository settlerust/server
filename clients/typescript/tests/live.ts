/** Optional real-server smoke test: node --experimental-strip-types tests/live.ts URL */
import assert from 'node:assert/strict';
import { SettlersClient } from '../src/index.ts';
const base = process.argv[2] ?? 'http://127.0.0.1:8080';
const host = await SettlersClient.guest(base, 'TypeScript host');
const peer = await SettlersClient.guest(base, 'TypeScript peer');
let game = await host.create({name: 'TypeScript protocol smoke', host_as_player: true,
  settings: {players: 2, points_to_win: 10, discard_limit: 7, turn_seconds: 120}});
await peer.submit(game.id, peer.prepare(game.version, {type: 'join', name: 'TypeScript peer', spectator: false}));
game = await host.game(game.id);
const request = host.prepare(game.version, {type: 'start'});
const receipt = await host.submit(game.id, request);
assert.equal((await host.submit(game.id, request)).duplicate, true);
await host.refresh();
let stop: (() => void) | undefined;
try {
  await new Promise<void>((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error('WebSocket snapshot timed out')), 10_000);
    stop = host.connect(game.id, frame => {
      if (frame.type === 'snapshot' && frame.game.game) {
        try {
          assert.equal(frame.protocol, 1);
          assert.equal(frame.game.game.players.length, 2);
          assert.ok(frame.game.version >= receipt.version);
          clearTimeout(timer);
          resolve();
        } catch (error) { clearTimeout(timer); reject(error); }
      }
    }, error => { clearTimeout(timer); reject(error); });
  });
} finally { stop?.(); }
const history = await host.history(game.id);
assert.ok(history.next_after >= receipt.version);
console.log(JSON.stringify({game_id: game.id, typescript_rest_and_websocket: 'passed'}));
