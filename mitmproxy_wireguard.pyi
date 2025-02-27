from typing import Awaitable, Callable

class Server:
    def getsockname(self) -> tuple[str, int]: ...
    async def new_connection(self, src_addr: tuple[str, int], dst_addr: tuple[str, int]) -> TcpStream: ...
    def send_datagram(self, data: bytes, src_addr: tuple[str, int], dst_addr: tuple[str, int]) -> None: ...
    def send_other_packet(self, data: bytes) -> None: ...
    def close(self) -> None: ...
    async def wait_closed(self) -> None: ...

class TcpStream:
    async def read(self, n: int) -> bytes: ...
    def write(self, data: bytes): ...
    async def drain(self) -> None: ...
    def write_eof(self): ...
    def close(self): ...
    def is_closing(self) -> bool: ...
    def get_extra_info(self, name: str) -> tuple[str, int]: ...
    def __repr__(self) -> str: ...

async def start_server(
    host: str,
    port: int,
    private_key: str,
    peer_public_keys: list[str],
    peer_endpoints: list[str|None],
    handle_connection: Callable[[TcpStream], Awaitable[None]],
    receive_datagram: Callable[[bytes, tuple[str, int], tuple[str, int]], Awaitable[None]],
    receive_other_packet: Callable[[bytes], Awaitable[None]],
) -> Server: ...
def genkey() -> str: ...
def pubkey(private_key: str) -> str: ...
