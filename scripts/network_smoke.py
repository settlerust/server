"""Exercise saved Rust policy credentials, Python/WS observations, full games, and process restart.
Requires a running server and a release build of the remote policy binary.
"""
import asyncio
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import time
from settlers_client import Client, ServerError
from jsonschema import Draft202012Validator

async def main():
    root = Path(__file__).resolve().parents[1]
    base = sys.argv[1] if len(sys.argv) > 1 else 'http://127.0.0.1:8080'
    host = Client.guest(base, 'Network smoke observer')
    lobby = host.create({'name': 'Four remote policies', 'host_as_player': False})
    game_id = lobby['id']
    schema = json.loads((root / 'docs/protocol.schema.json').read_text())
    frame_validator = Draft202012Validator({'$ref': '#/$defs/ServerFrame', '$defs': schema['$defs']})
    with tempfile.TemporaryDirectory(prefix='settlers-policies-') as temporary:
        log_path = Path(temporary) / 'policies.log'
        with log_path.open('w+') as log:
            processes = []
            try:
                for p in range(4):
                    processes.append(subprocess.Popen([str(root/'target/release/remote'),base,game_id,'fast' if p%2 == 0 else 'eta',str(Path(temporary)/f'{p}.jwt'),f'Policy {p}'],stdout=log,stderr=log))
                deadline = time.monotonic() + 30
                while time.monotonic() < deadline:
                    view = host.game(game_id)
                    if len([m for m in view['members'] if m['seat'] is not None and m['connected']]) == 4:
                        break
                    if any(p.poll() is not None for p in processes):
                        raise RuntimeError('policy exited before start')
                    await asyncio.sleep(.1)
                else:
                    raise RuntimeError('policies did not join/connect')
                # Restart one policy with the same saved JWT before starting the match.
                saved = Client(base, (Path(temporary) / '0.jwt').read_text())
                original_seat = next(m['seat'] for m in saved.game(game_id)['members'] if m['is_you'])
                processes[0].terminate()
                processes[0].wait(timeout=10)
                processes[0] = subprocess.Popen([str(root/'target/release/remote'),base,game_id,'fast',str(Path(temporary)/'0.jwt'),'Policy 0'],stdout=log,stderr=log)
                deadline = time.monotonic() + 30
                while time.monotonic() < deadline:
                    own = next(m for m in saved.game(game_id)['members'] if m['is_you'])
                    assert own['seat'] == original_seat
                    if own['connected']:
                        break
                    await asyncio.sleep(.1)
                while True:
                    view = host.game(game_id)
                    try:
                        host.submit(game_id,host.prepare(view['version'],{'type':'start'}))
                        break
                    except ServerError as error:
                        if error.data['code'] != 'conflict':
                            raise
                stream = host.stream(game_id)
                frames = 0
                async with asyncio.timeout(90):
                    async for frame in stream:
                        frame_validator.validate(frame)
                        frames += 1
                        if frame['type'] == 'snapshot' and frame['game']['game']['phase']['type'] == 'finished':
                            result = frame['game']
                            break
                await stream.aclose()
                after = 0
                count = 0
                while after < result['version']:
                    page = host.history(game_id,after,100)
                    assert page['next_after'] > after
                    after = page['next_after']
                    count += len(page['events'])
                print(json.dumps({'game_id':game_id,'phase':result['game']['phase'],'turns':result['game']['turn'],'frames_validated':frames,'history_batches':count}))
                # Reusing a persisted identity must resolve to the same seat after a process restart.
                p0 = Client(base,(Path(temporary)/'0.jwt').read_text())
                assert next(m['seat'] for m in p0.game(game_id)['members'] if m['is_you']) == original_seat
            except BaseException:
                log.flush()
                print(log_path.read_text()[-6000:], file=sys.stderr)
                raise
            finally:
                for process in processes:
                    if process.poll() is None:
                        process.terminate()
                for process in processes:
                    process.wait(timeout=10)

if __name__ == '__main__':
    asyncio.run(main())
