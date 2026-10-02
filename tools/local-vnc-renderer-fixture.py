"""Disposable no-auth RFB 3.8 fixture, hard-bound to loopback; no files or shell access.

Run from any directory. Printed JSON reports the ephemeral port and observed
input. Ctrl+C/SIGTERM stops it; SIGUSR1 disconnects its client for reconnect QA.
This is a controlled protocol fixture, not a general-purpose VNC server.
"""
import json
import select
import signal
import socket
import struct
import time

WIDTH, HEIGHT = 1920, 1080
phase = 0
active = None


def report(**fields):
    print(json.dumps(fields), flush=True)


def read_exact(stream, length):
    data = bytearray()
    while len(data) < length:
        chunk = stream.recv(length - len(data))
        if not chunk:
            raise EOFError()
        data.extend(chunk)
    return data


def disconnect(*_):
    global phase
    phase += 1
    if active is not None:
        try:
            active.shutdown(socket.SHUT_RDWR)
        except OSError:
            pass


def stop(*_):
    raise KeyboardInterrupt()


def serve(stream):
    global phase
    stream.settimeout(5)
    stream.sendall(b"RFB 003.008\n")
    assert read_exact(stream, 12) == b"RFB 003.008\n"
    stream.sendall(bytes([1, 1]))
    assert read_exact(stream, 1) == bytes([1])
    stream.sendall(bytes(4))
    read_exact(stream, 1)
    name = b"Full-HD loopback renderer fixture"
    pixel_format = bytes([32, 24, 0, 1, 0, 255, 0, 255, 0, 255, 0, 0, 0, 8, 0, 16])
    stream.sendall(struct.pack(">HH", WIDTH, HEIGHT) + pixel_format + struct.pack(">I", len(name)) + name)
    header = bytes([0, 0, 0, 1]) + struct.pack(">HHHHi", 0, 0, WIDTH, HEIGHT, 0)
    pending = False
    last_frame = 0
    cached_phase = None
    pixels = b""
    frames = 0
    while True:
        if select.select([stream], [], [], 0.02)[0]:
            kind = read_exact(stream, 1)[0]
            if kind == 0:
                read_exact(stream, 19)
            elif kind == 2:
                count = struct.unpack(">H", read_exact(stream, 3)[1:])[0]
                assert count <= 32
                read_exact(stream, count * 4)
            elif kind == 3:
                read_exact(stream, 9)
                pending = True
            elif kind == 4:
                data = read_exact(stream, 7)
                if data[0]:
                    phase += 1
                    report(event="key", keysym=struct.unpack(">I", data[3:])[0], phase=phase, frames=frames)
            elif kind == 5:
                data = read_exact(stream, 5)
                if data[0]:
                    phase += 1
                    report(event="pointer", buttons=data[0], x=struct.unpack(">H", data[1:3])[0], y=struct.unpack(">H", data[3:])[0], phase=phase)
            elif kind == 6:
                length = struct.unpack(">I", read_exact(stream, 7)[3:])[0]
                assert length <= 1024 * 1024
                read_exact(stream, length)
                report(event="clipboard", bytes=length)
            else:
                raise ValueError("unexpected client message")
        if pending and time.monotonic() - last_frame >= 0.2:
            if cached_phase != phase:
                left = bytes([35, 103, 171, 255]) if phase % 2 == 0 else bytes([68, 180, 102, 255])
                right = bytes([220, 120, 35, 255]) if phase % 2 == 0 else bytes([155, 80, 190, 255])
                row = left * (WIDTH // 2) + right * (WIDTH // 2)
                pixels = row * HEIGHT
                cached_phase = phase
            stream.sendall(header)
            stream.sendall(pixels)
            frames += 1
            if frames == 1:
                report(event="frame", width=WIDTH, height=HEIGHT, bytes=len(pixels), phase=phase)
            last_frame = time.monotonic()
            pending = False


if __name__ == "__main__":
    signal.signal(signal.SIGTERM, stop)
    signal.signal(signal.SIGUSR1, disconnect)
    with socket.socket() as listener:
        listener.bind(("127.0.0.1", 0))
        listener.listen(1)
        report(event="listening", host="127.0.0.1", port=listener.getsockname()[1])
        try:
            while True:
                active, address = listener.accept()
                assert address[0] == "127.0.0.1"
                with active:
                    try:
                        serve(active)
                    except (EOFError, OSError, AssertionError, ValueError):
                        report(event="disconnected")
                active = None
        except KeyboardInterrupt:
            pass
        finally:
            if active is not None:
                active.close()
