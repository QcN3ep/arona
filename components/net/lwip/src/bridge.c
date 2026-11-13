// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

#include "lwip/tcpip.h"
#include "lwip/sockets.h"
#include "lwip/errno.h"
#include "lwip/sys.h"
#include <string.h>

struct arona_lwip_endpoint {
  uint8_t address[16];
  uint16_t port;
  uint16_t family;
};
_Static_assert(sizeof(struct arona_lwip_endpoint) == 20, "Rust endpoint size");
_Static_assert(_Alignof(struct arona_lwip_endpoint) == 2, "Rust endpoint alignment");
_Static_assert(sizeof(ssize_t) == sizeof(ptrdiff_t), "Rust isize result");

void arona_lwip_seed(const uint8_t *seed);
int arona_lwip_prepare_teardown(void);
void arona_lwip_teardown(void *argument);
void arona_lwip_received(int fd, int eof);

static void ready(void *argument) {
  sys_sem_signal((sys_sem_t *)argument);
}

int arona_lwip_start(const uint8_t *seed) {
  arona_lwip_seed(seed);
  sys_sem_t initialized = SYS_SEM_NULL;
  if (sys_sem_new(&initialized, 0) != ERR_OK) { return -1; }
  err_t result = tcpip_init(ready, &initialized);
  if (result == ERR_OK) { sys_sem_wait(&initialized); }
  sys_sem_free(&initialized);
  if (result != ERR_OK || arona_lwip_prepare_teardown() != 0) { return -1; }
  return sys_thread_new("teardown", arona_lwip_teardown, NULL, 128 * 1024, 0) ? 0 : -1;
}

int arona_lwip_last_error(void) { return errno; }

union socket_address {
  struct sockaddr base;
  struct sockaddr_in v4;
  struct sockaddr_in6 v6;
};

static socklen_t address(union socket_address *value, const struct arona_lwip_endpoint *input) {
  memset(value, 0, sizeof(*value));
  if (input->family == 6) {
    value->v6.sin6_len = sizeof(value->v6);
    value->v6.sin6_family = AF_INET6;
    value->v6.sin6_port = lwip_htons(input->port);
    memcpy(&value->v6.sin6_addr, input->address, 16);
    return sizeof(value->v6);
  }
  value->v4.sin_len = sizeof(value->v4);
  value->v4.sin_family = AF_INET;
  value->v4.sin_port = lwip_htons(input->port);
  memcpy(&value->v4.sin_addr.s_addr, input->address, 4);
  return sizeof(value->v4);
}

static void endpoint(struct arona_lwip_endpoint *output, const union socket_address *value) {
  memset(output, 0, sizeof(*output));
  if (value->base.sa_family == AF_INET6) {
    memcpy(output->address, &value->v6.sin6_addr, 16);
    output->port = lwip_ntohs(value->v6.sin6_port);
    output->family = 6;
  } else {
    memcpy(output->address, &value->v4.sin_addr.s_addr, 4);
    output->port = lwip_ntohs(value->v4.sin_port);
    output->family = 4;
  }
}

static int nonblocking(int fd, int family) {
  if (fd < 0) { return fd; }
  int enabled = 1;
  if (lwip_fcntl(fd, F_SETFL, O_NONBLOCK) < 0 ||
      (family == AF_INET6 && lwip_setsockopt(fd, IPPROTO_IPV6, IPV6_V6ONLY,
                                           &enabled, sizeof(enabled)) < 0)) {
    int saved = errno;
    lwip_close(fd);
    errno = saved;
    return -1;
  }
  return fd;
}

int arona_lwip_socket(int kind, int family) {
  int domain = family == 6 ? AF_INET6 : AF_INET;
  return nonblocking(lwip_socket(domain, kind, 0), domain);
}

int arona_lwip_bind(int fd, const struct arona_lwip_endpoint *peer) {
  union socket_address value;
  socklen_t length = address(&value, peer);
  return lwip_bind(fd, &value.base, length);
}

int arona_lwip_listen(int fd, int backlog) { return lwip_listen(fd, backlog); }

int arona_lwip_connect(int fd, const struct arona_lwip_endpoint *peer) {
  union socket_address value;
  socklen_t length = address(&value, peer);
  return lwip_connect(fd, &value.base, length);
}

int arona_lwip_disconnect(int fd) {
  struct sockaddr value;
  memset(&value, 0, sizeof(value));
  value.sa_len = sizeof(value);
  value.sa_family = AF_UNSPEC;
  return lwip_connect(fd, &value, sizeof(value));
}

int arona_lwip_accept(int fd, struct arona_lwip_endpoint *peer) {
  union socket_address value;
  socklen_t length = sizeof(value);
  int accepted = lwip_accept(fd, &value.base, &length);
  if (accepted >= 0) { accepted = nonblocking(accepted, value.base.sa_family); }
  if (accepted >= 0) { endpoint(peer, &value); }
  return accepted;
}

int arona_lwip_endpoint(int fd, struct arona_lwip_endpoint *output, int peer) {
  union socket_address value;
  socklen_t length = sizeof(value);
  int result = peer ? lwip_getpeername(fd, (struct sockaddr *)&value, &length)
                    : lwip_getsockname(fd, (struct sockaddr *)&value, &length);
  if (result == 0) { endpoint(output, &value); }
  return result;
}

ssize_t arona_lwip_send(int fd, const uint8_t *bytes, size_t length,
                       const struct arona_lwip_endpoint *peer) {
  if (!peer) { return lwip_send(fd, bytes, length, 0); }
  union socket_address value;
  socklen_t size = address(&value, peer);
  return lwip_sendto(fd, bytes, length, 0, &value.base, size);
}

ssize_t arona_lwip_receive(int fd, uint8_t *bytes, size_t length,
                          struct arona_lwip_endpoint *peer) {
  if (!peer) {
    ssize_t result = lwip_recv(fd, bytes, length, 0);
    arona_lwip_received(fd, result == 0 && length != 0);
    return result;
  }
  union socket_address value;
  socklen_t size = sizeof(value);
  ssize_t result = lwip_recvfrom(fd, bytes, length, 0, (struct sockaddr *)&value, &size);
  arona_lwip_received(fd, result == 0 && length != 0);
  if (result >= 0 && size != 0) { endpoint(peer, &value); }
  return result;
}

int arona_lwip_take_error(int fd) {
  int error = 0;
  socklen_t size = sizeof(error);
  if (lwip_getsockopt(fd, SOL_SOCKET, SO_ERROR, &error, &size) < 0) { return -1; }
  if (error) { errno = error; return -1; }
  return 0;
}

int arona_lwip_shutdown(int fd, int direction) { return lwip_shutdown(fd, direction); }
int arona_lwip_close(int fd) { return lwip_close(fd); }

int arona_lwip_option(int fd, int option, int *value, int set) {
  int level = IPPROTO_TCP;
  int name;
  switch (option) {
    case 0: level = SOL_SOCKET; name = SO_REUSEADDR; break;
    case 1: name = TCP_NODELAY; break;
    case 2: level = SOL_SOCKET; name = SO_KEEPALIVE; break;
    case 3: name = TCP_KEEPIDLE; break;
    case 4: name = TCP_KEEPINTVL; break;
    case 5: name = TCP_KEEPCNT; break;
    case 8: level = IPPROTO_IPV6; name = IPV6_V6ONLY; break;
    case 6:
    case 7: {
      int type;
      socklen_t size = sizeof(type);
      if (lwip_getsockopt(fd, SOL_SOCKET, SO_TYPE, &type, &size) < 0) { return -1; }
      if (option == 7 || type == SOCK_STREAM) {
        if (set) { errno = EOPNOTSUPP; return -1; }
        if (type == SOCK_STREAM) { *value = option == 6 ? TCP_WND : TCP_SND_BUF; }
        else {
          union socket_address local;
          socklen_t length = sizeof(local);
          if (lwip_getsockname(fd, &local.base, &length) < 0) { return -1; }
          *value = local.base.sa_family == AF_INET6 ? 65487 : 65507;
        }
        return 0;
      }
      level = SOL_SOCKET; name = SO_RCVBUF; break;
    }
    default: errno = ENOPROTOOPT; return -1;
  }
  if (set) { return lwip_setsockopt(fd, level, name, value, sizeof(*value)); }
  socklen_t size = sizeof(*value);
  return lwip_getsockopt(fd, level, name, value, &size);
}
