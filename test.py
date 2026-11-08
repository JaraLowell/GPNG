import socket
import threading
import sys

# Configuration
PORT_A = 8100
PORT_B = 8200
HOST = '127.0.0.1'

def hex_dump(data, direction):
    """Prints readable hex bytes of the KISS frames passing through."""
    hex_str = ' '.join(f'{b:02X}' for b in data)
    print(f"[{direction}] ({len(data)} bytes): {hex_str}")
    sys.stdout.flush()

def handle_forward(source_sock, dest_sock, direction_label):
    """Forwards data from one socket to another and logs it."""
    try:
        while True:
            data = source_sock.recv(4096)
            if not data:
                print(f"[-] Connection closed on side: {direction_label.split('->')[0]}")
                break
            
            # Monitor the raw traffic
            hex_dump(data, direction_label)
            
            # Forward to the other instance
            dest_sock.sendall(data)
    except Exception as e:
        print(f"[!] Error in {direction_label}: {e}")
    finally:
        try: source_sock.close() 
        except: pass
        try: dest_sock.close() 
        except: pass

def main():
    # Setup listener A
    server_a = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    server_a.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    server_a.bind((HOST, PORT_A))
    server_a.listen(1)
    print(f"[*] Port A: Waiting for Instance 1 on {HOST}:{PORT_A}...")

    # Setup listener B
    server_b = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    server_b.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    server_b.bind((HOST, PORT_B))
    server_b.listen(1)
    print(f"[*] Port B: Waiting for Instance 2 on {HOST}:{PORT_B}...")

    # Accept both connections
    conn_a, addr_a = server_a.accept()
    print(f"[+] Instance 1 connected from {addr_a}")
    
    conn_b, addr_b = server_b.accept()
    print(f"[+] Instance 2 connected from {addr_b}")

    # Close listening sockets to prevent other connections
    server_a.close()
    server_b.close()

    # Start bi-directional forwarding threads
    t1 = threading.Thread(target=handle_forward, args=(conn_a, conn_b, "Inst1 -> Inst2"))
    t2 = threading.Thread(target=handle_forward, args=(conn_b, conn_a, "Inst2 -> Inst1"))
    
    t1.start()
    t2.start()

    t1.join()
    t2.join()
    print("[*] Bridge terminated.")

if __name__ == "__main__":
    main()
