"""TCP bridge between two KISS clients (ports 8100 and 8200).

Either side can disconnect. That socket is dropped and the bridge waits for
it to come back; the other side stays connected, and the process keeps
listening. Ctrl+C stops it.
"""

import select
import socket
import threading

# Configuration
PORT_A = 8100
PORT_B = 8200
HOST = "127.0.0.1"

# Windows reports these when the peer goes away, or when this process closes
# a socket another thread is blocked on. They are not fatal to the bridge.
_DISCONNECT_WINERRORS = {10038, 10053, 10054, 10057, 10058}

_SIDES = {
    "Inst1": ("Instance 1", "A", PORT_A),
    "Inst2": ("Instance 2", "B", PORT_B),
}


def log(msg):
    print(msg, flush=True)


def hex_dump(data, direction):
    """Prints readable hex bytes of the KISS frames passing through."""
    hex_str = " ".join(f"{b:02X}" for b in data)
    log(f"[{direction}] ({len(data)} bytes): {hex_str}")


def is_disconnect(exc):
    if isinstance(exc, (ConnectionResetError, ConnectionAbortedError, BrokenPipeError)):
        return True
    return getattr(exc, "winerror", None) in _DISCONNECT_WINERRORS


def close_quietly(sock):
    if sock is None:
        return
    try:
        sock.shutdown(socket.SHUT_RDWR)
    except OSError:
        pass
    try:
        sock.close()
    except OSError:
        pass


def drain(sock):
    while True:
        try:
            if not sock.recv(4096):
                return
        except OSError:
            return


class Bridge:
    """Two KISS TCP clients. Either socket can drop without stopping the process."""

    def __init__(self):
        self.cv = threading.Condition()
        self.conn = {"Inst1": None, "Inst2": None}
        self.stop = False
        # One wake pipe per forward thread so a disconnect unblocks select
        # without pulling KISS bytes off the live socket.
        self._wake_w = []
        self.wake_r = {}
        for key in ("ab", "ba"):
            rd, wr = socket.socketpair()
            rd.setblocking(False)
            wr.setblocking(False)
            self.wake_r[key] = rd
            self._wake_w.append(wr)

    def poke(self):
        for wr in self._wake_w:
            try:
                wr.send(b"x")
            except OSError:
                pass

    def set_conn(self, name, sock):
        with self.cv:
            old = self.conn[name]
            self.conn[name] = sock
            self.cv.notify_all()
        self.poke()
        if old is not None and old is not sock:
            close_quietly(old)

    def take(self, name, sock):
        """Detach sock if it is still current. Caller closes it."""
        with self.cv:
            if self.conn[name] is not sock:
                return None
            self.conn[name] = None
            self.cv.notify_all()
        self.poke()
        return sock

    def close_wakes(self):
        for sock in self._wake_w:
            close_quietly(sock)
        for sock in self.wake_r.values():
            close_quietly(sock)


def accept_loop(server, name, bridge):
    title, _, _ = _SIDES[name]
    while True:
        try:
            server.settimeout(0.5)
            conn, addr = server.accept()
        except socket.timeout:
            with bridge.cv:
                if bridge.stop:
                    return
            continue
        except OSError as e:
            with bridge.cv:
                if bridge.stop:
                    return
            log(f"[!] Accept error for {title}: {e}")
            return
        conn.settimeout(None)
        bridge.set_conn(name, conn)
        log(f"[+] {title} connected from {addr}")


def note_closed(name):
    title, side, port = _SIDES[name]
    log(f"[-] Connection closed on side: {title}")
    log(f"[*] Port {side}: Waiting for {title} on {HOST}:{port}...")


def drop_side(bridge, name, sock, label, exc):
    if bridge.take(name, sock) is None:
        return
    close_quietly(sock)
    with bridge.cv:
        # A new client may already own this slot. Announcing a close after
        # that [+] line would blame the live connection.
        if bridge.stop or bridge.conn[name] is not None:
            return
    if exc is None or is_disconnect(exc):
        note_closed(name)
    else:
        log(f"[!] Error in {label}: {exc}")
        note_closed(name)


def wait_for_dest(src, src_name, dst_name, wake_r, bridge):
    """Block while the destination is down.

    A source that closes is reported without pulling queued KISS bytes off
    the socket. Those stay in the kernel buffer until the other side returns.
    """
    while True:
        with bridge.cv:
            if bridge.stop:
                return False
            if bridge.conn[src_name] is not src:
                return True
            if bridge.conn[dst_name] is not None:
                return True
        try:
            readable, _, _ = select.select([src, wake_r], [], [])
        except (OSError, ValueError):
            return False
        if wake_r in readable:
            drain(wake_r)
            continue
        try:
            peek = src.recv(1, socket.MSG_PEEK)
        except (OSError, ValueError):
            return False
        if not peek:
            return False
        # Payload is waiting and the other side is still down. Park on the
        # wake pipe so a readable socket does not turn into a spin.
        try:
            readable, _, _ = select.select([wake_r], [], [])
        except (OSError, ValueError):
            return False
        drain(wake_r)


def handle_forward(src_name, dst_name, label, wake_r, bridge):
    """Copy bytes one way. A dead peer is dropped; the other side stays up."""
    while True:
        with bridge.cv:
            while not bridge.stop and bridge.conn[src_name] is None:
                bridge.cv.wait(0.5)
            if bridge.stop:
                return
            src = bridge.conn[src_name]
            dst_up = bridge.conn[dst_name] is not None
        if not dst_up:
            if not wait_for_dest(src, src_name, dst_name, wake_r, bridge):
                with bridge.cv:
                    if bridge.stop:
                        return
                drop_side(bridge, src_name, src, label, None)
            continue
        try:
            readable, _, _ = select.select([src, wake_r], [], [])
        except (OSError, ValueError) as e:
            with bridge.cv:
                if bridge.stop:
                    return
            drop_side(bridge, src_name, src, label, e)
            continue
        if wake_r in readable:
            drain(wake_r)
        if src not in readable:
            continue
        with bridge.cv:
            if bridge.stop:
                return
            # Leave bytes queued until this socket and a live destination
            # are still the pair we are forwarding for.
            if bridge.conn[src_name] is not src or bridge.conn[dst_name] is None:
                continue
        try:
            data = src.recv(4096)
        except (OSError, ValueError) as e:
            with bridge.cv:
                if bridge.stop:
                    return
            drop_side(bridge, src_name, src, label, e)
            continue
        if not data:
            with bridge.cv:
                if bridge.stop:
                    return
            drop_side(bridge, src_name, src, label, None)
            continue
        hex_dump(data, label)
        while True:
            with bridge.cv:
                if bridge.stop:
                    return
                if bridge.conn[src_name] is not src:
                    break
                dst = bridge.conn[dst_name]
                if dst is None:
                    bridge.cv.wait(0.5)
                    continue
            try:
                dst.sendall(data)
            except OSError as e:
                with bridge.cv:
                    if bridge.stop:
                        return
                # sendall may have written a prefix; do not send the chunk again.
                drop_side(bridge, dst_name, dst, label, e)
            break


def listen(port):
    server = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    server.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    server.bind((HOST, port))
    server.listen(5)
    return server


def main():
    bridge = Bridge()
    server_a = server_b = None
    try:
        server_a = listen(PORT_A)
        server_b = listen(PORT_B)
    except OSError as e:
        log(f"[!] Could not listen on {HOST}:{PORT_A}/{PORT_B}: {e}")
        close_quietly(server_a)
        close_quietly(server_b)
        bridge.close_wakes()
        return

    log(f"[*] Port A: Waiting for Instance 1 on {HOST}:{PORT_A}...")
    log(f"[*] Port B: Waiting for Instance 2 on {HOST}:{PORT_B}...")

    threads = [
        threading.Thread(target=accept_loop, args=(server_a, "Inst1", bridge), name="accept-a"),
        threading.Thread(target=accept_loop, args=(server_b, "Inst2", bridge), name="accept-b"),
        threading.Thread(
            target=handle_forward,
            args=("Inst1", "Inst2", "Inst1 -> Inst2", bridge.wake_r["ab"], bridge),
            name="fwd-a",
        ),
        threading.Thread(
            target=handle_forward,
            args=("Inst2", "Inst1", "Inst2 -> Inst1", bridge.wake_r["ba"], bridge),
            name="fwd-b",
        ),
    ]
    for thread in threads:
        thread.daemon = True
        thread.start()

    try:
        while True:
            threading.Event().wait(3600)
    except KeyboardInterrupt:
        log("\n[*] Bridge stopped.")
    finally:
        with bridge.cv:
            bridge.stop = True
            conns = list(bridge.conn.values())
            bridge.conn["Inst1"] = None
            bridge.conn["Inst2"] = None
            bridge.cv.notify_all()
        bridge.poke()
        close_quietly(server_a)
        close_quietly(server_b)
        for conn in conns:
            close_quietly(conn)
        bridge.close_wakes()
        for thread in threads:
            thread.join(timeout=2)


if __name__ == "__main__":
    main()
