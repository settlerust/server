"""Typed REST client and reconnecting WebSocket iterator. Keep the same token across runs."""
import json
from urllib.request import Request, urlopen
from urllib.error import HTTPError
from urllib.parse import quote
from uuid import uuid4
from typing import AsyncIterator, Any, cast
from .types import Command, CommandRequest, CreateGame, EventPage, GameView, Receipt, ServerFrame, TokenResponse

class ServerError(Exception):
    def __init__(self, data: dict[str, Any]):
        super().__init__(data['message'])
        self.data = data

class Client:
    def __init__(self, base: str, token: str = ''):
        self.base = base.rstrip('/')
        self.token = token

    @classmethod
    def guest(cls, base: str, name: str) -> 'Client':
        client = cls(base)
        client.token = client._request('/v1/auth/guest', {'name': name})['token']
        return client

    def _request(self, path: str, body: Any = None, *, post: bool = False) -> Any:
        data = json.dumps(body).encode() if body is not None or post else None
        request = Request(self.base + path, data=data, headers={'Authorization': 'Bearer ' + self.token, 'Content-Type': 'application/json'})
        try:
            with urlopen(request, timeout=30) as response:
                return json.load(response)
        except HTTPError as error:
            raise ServerError(json.load(error)) from error

    def refresh(self) -> TokenResponse:
        response = self._request('/v1/auth/refresh', post=True)
        self.token = response['token']
        return cast(TokenResponse, response)

    def create(self, request: CreateGame) -> GameView:
        return cast(GameView, self._request('/v1/games', request))

    def game(self, game_id: str) -> GameView:
        return cast(GameView, self._request('/v1/games/' + quote(game_id, safe='')))

    @staticmethod
    def prepare(version: int, command: Command) -> CommandRequest:
        return {'request_id': str(uuid4()), 'expected_version': version, 'command': command}

    def submit(self, game_id: str, request: CommandRequest) -> Receipt:
        """Resend the exact request after transport failure to avoid duplicate actions."""
        return cast(Receipt, self._request('/v1/games/' + quote(game_id, safe='') + '/commands', request))

    def history(self, game_id: str, after: int = 0, limit: int = 100) -> EventPage:
        return cast(EventPage, self._request(f'/v1/games/{quote(game_id, safe="")}/events?after={after}&limit={limit}'))

    async def stream(self, game_id: str) -> AsyncIterator[ServerFrame]:
        from websockets.asyncio.client import connect
        from websockets.exceptions import ConnectionClosed
        from websockets.typing import Subprotocol
        url = self.base.replace('http', 'ws', 1) + '/v1/games/' + quote(game_id, safe='') + '/ws'
        async for socket in connect(url, additional_headers={'Authorization': 'Bearer ' + self.token}, subprotocols=[Subprotocol('settlers-v1')], max_size=1_048_576):
            try:
                async for message in socket:
                    yield cast(ServerFrame, json.loads(message))
            except ConnectionClosed:
                continue
