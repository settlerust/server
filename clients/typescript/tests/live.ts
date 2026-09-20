/** Optional real-server smoke test: node --experimental-strip-types tests/live.ts URL */
import assert from 'node:assert/strict';
import { SettlersClient } from '../src/index.ts';
const base = process.argv[2] ?? 'http://127.0.0.1:8080';
const host = await SettlersClient.guest(base, 'TypeScript host');
const peer = await SettlersClient.guest(base, 'TypeScript peer');
let game = await host.create({name: 'TypeScript protocol smoke', host_as_player: true,
  settings: {players: 2, points_to_win: 10, discard_limit: 7, turn_seconds: 120}});
await peer.submit(game.id, peer.prepare(game.version, {type: 'join', name: 'TypeScript peer', spectator: false}));
const stops: (() => void)[] = [];
try {
  // The real server requires both a live WebSocket and explicit player consent.
  await Promise.all([host, peer].map(client => new Promise<void>((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error('Player presence timed out')), 10_000);
    stops.push(client.connect(game.id, frame => {
      if (frame.type === 'snapshot' && frame.game.members.filter(member => member.seat != null).every(member => member.connected)) {
        clearTimeout(timer);
        resolve();
      }
    }, error => { clearTimeout(timer); reject(error); }));
  })));
  for (const client of [host, peer]) {
    const current = await client.game(game.id);
    await client.submit(game.id, client.prepare(current.version, {type: 'set_ready', ready: true}));
  }
  game = await host.game(game.id);
  const request = host.prepare(game.version, {type: 'start'});
  const receipt = await host.submit(game.id, request);
  assert.equal((await host.submit(game.id, request)).duplicate, true);
  await host.refresh();
  await new Promise<void>((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error('WebSocket snapshot timed out')), 10_000);
    stops.push(host.connect(game.id, frame => {
      if (frame.type === 'snapshot' && frame.game.game) {
        try {
          assert.equal(frame.protocol, 1);
          assert.equal(frame.game.game.players.length, 2);
          assert.ok(frame.game.version >= receipt.version);
          clearTimeout(timer);
          resolve();
        } catch (error) { clearTimeout(timer); reject(error); }
      }
    }, error => { clearTimeout(timer); reject(error); }));
  });
  const history = await host.history(game.id);
  assert.ok(history.next_after >= receipt.version);
  console.log(JSON.stringify({game_id: game.id, typescript_rest_and_websocket: 'passed'}));
} finally { for (const stop of stops) stop(); }
