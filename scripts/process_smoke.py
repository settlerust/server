"""Run a disposable server, three-language network checks, and abrupt crash recovery.
Requires release binaries, Node 24, and `pip install -e clients/python jsonschema`.
"""
import json
import os
from pathlib import Path
import socket
import subprocess
import sys
import tempfile
import time
from settlers_client import Client


def main():
    root = Path(__file__).resolve().parents[1]
    with socket.socket() as reservation:
        reservation.bind(('127.0.0.1', 0))
        port = reservation.getsockname()[1]
    base = f'http://127.0.0.1:{port}'
    with tempfile.TemporaryDirectory(prefix='settlers-process-smoke-') as temporary:
        env = {**os.environ, 'SETTLERS_MODE': 'local', 'SETTLERS_STORAGE': 'local',
               'SETTLERS_BIND': f'127.0.0.1:{port}', 'SETTLERS_DATA_DIR': temporary}
        env.pop('SETTLERS_JWT_SECRET', None)
        log_path = Path(temporary) / 'server.log'
        with log_path.open('w+') as log:
            process = None
            def start():
                child = subprocess.Popen([str(root/'target/release/server')], env=env, stdout=log, stderr=log)
                deadline = time.monotonic() + 15
                while time.monotonic() < deadline:
                    if child.poll() is not None:
                        raise RuntimeError('server exited during startup')
                    try:
                        with socket.create_connection(('127.0.0.1', port), timeout=.2):
                            return child
                    except OSError:
                        time.sleep(.05)
                child.kill()
                child.wait()
                raise RuntimeError('server failed to listen')
            try:
                process = start()
                subprocess.run([sys.executable, str(root/'scripts/network_smoke.py'), base], check=True, timeout=150)
                subprocess.run(['node', '--experimental-strip-types', str(root/'clients/typescript/tests/live.ts'), base], check=True, timeout=20)
                host = Client.guest(base, 'Crash recovery host')
                peer = Client.guest(base, 'Crash recovery peer')
                view = host.create({'name': 'Crash recovery', 'host_as_player': True,
                                    'settings': {'players': 2, 'points_to_win': 10, 'discard_limit': 7, 'turn_seconds': 86400}})
                game_id = view['id']
                peer.submit(game_id, peer.prepare(view['version'], {'type': 'join', 'name': 'Peer', 'spectator': False}))
                view = host.game(game_id)
                host.submit(game_id, host.prepare(view['version'], {'type': 'start'}))
                view = host.game(game_id)
                actor = [host, peer][view['game']['current']]
                own = actor.game(game_id)
                action = own['game']['legal_actions'][0]
                request = actor.prepare(own['version'], {'type': 'act', 'action': action})
                receipt = actor.submit(game_id, request)
                before = host.game(game_id)
                process.kill()  # No graceful shutdown or final archive flush.
                process.wait(timeout=10)
                process = start()
                after = host.game(game_id)  # The same JWT must still authenticate.
                assert before == after, 'durable game changed across abrupt crash'
                retried = actor.submit(game_id, request)
                assert retried['duplicate'] and retried['version'] == receipt['version']
                assert host.history(game_id)['next_after'] == after['version']
                print(json.dumps({'crash_recovery': 'passed', 'saved_jwt': 'passed', 'idempotent_retry': 'passed'}))
            except BaseException:
                log.flush()
                print(log_path.read_text()[-6000:], file=sys.stderr)
                raise
            finally:
                if process is not None and process.poll() is None:
                    process.kill()
                    process.wait(timeout=10)


if __name__ == '__main__':
    main()
