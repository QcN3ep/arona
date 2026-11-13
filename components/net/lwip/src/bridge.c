// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

#include "lwip/tcpip.h"
#include "lwip/sockets.h"
#include "lwip/errno.h"
#include "lwip/sys.h"
#include <string.h>

struct arona_lwip_endpoint {
  uint8_t address[4];
  uint16_t port;
};
_Static_assert(sizeof(struct arona_lwip_endpoint) == 6, "Rust endpoint size");
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

static struct sockaddr_in address(const struct arona_lwip_endpoint *endpoint) {
  struct sockaddr_in value;
  memset(&value, 0, sizeof(value));
  value.sin_len = sizeof(value);
  value.sin_family = AF_INET;
  value.sin_port = lwip_htons(endpoint->port);
  memcpy(&value.sin_addr.s_addr, endpoint->address, 4);
  return value;
}

static void endpoint(struct arona_lwip_endpoint *output, const struct sockaddr_in *value) {
  memcpy(output->address, &value->sin_addr.s_addr, 4);
  output->port = lwip_ntohs(value->sin_port);
}

static int nonblocking(int fd) {
  if (fd >= 0 && lwip_fcntl(fd, F_SETFL, O_NONBLOCK) < 0) {
    int error = errno;
    lwip_close(fd);
    errno = error;
    return -1;
  }
  return fd;
}

int arona_lwip_socket(int kind) {
  return nonblocking(lwip_socket(AF_INET, kind, 0));
}

int arona_lwip_bind(int fd, const struct arona_lwip_endpoint *peer) {
  struct sockaddr_in value = address(peer);
  return lwip_bind(fd, (const struct sockaddr *)&value, sizeof(value));
}

int arona_lwip_listen(int fd, int backlog) { return lwip_listen(fd, backlog); }

int arona_lwip_connect(int fd, const struct arona_lwip_endpoint *peer) {
  struct sockaddr_in value = address(peer);
  return lwip_connect(fd, (const struct sockaddr *)&value, sizeof(value));
}

int arona_lwip_disconnect(int fd) {
  struct sockaddr value;
  memset(&value, 0, sizeof(value));
  value.sa_len = sizeof(value);
  value.sa_family = AF_UNSPEC;
  return lwip_connect(fd, &value, sizeof(value));
}

int arona_lwip_accept(int fd, struct arona_lwip_endpoint *peer) {
  struct sockaddr_in value;
  socklen_t length = sizeof(value);
  int accepted = nonblocking(lwip_accept(fd, (struct sockaddr *)&value, &length));
  if (accepted >= 0) { endpoint(peer, &value); }
  return accepted;
}

int arona_lwip_endpoint(int fd, struct arona_lwip_endpoint *output, int peer) {
  struct sockaddr_in value;
  socklen_t length = sizeof(value);
  int result = peer ? lwip_getpeername(fd, (struct sockaddr *)&value, &length)
                    : lwip_getsockname(fd, (struct sockaddr *)&value, &length);
  if (result == 0) { endpoint(output, &value); }
  return result;
}

ssize_t arona_lwip_send(int fd, const uint8_t *bytes, size_t length,
                       const struct arona_lwip_endpoint *peer) {
  if (!peer) { return lwip_send(fd, bytes, length, 0); }
  struct sockaddr_in value = address(peer);
  return lwip_sendto(fd, bytes, length, 0, (const struct sockaddr *)&value, sizeof(value));
}

ssize_t arona_lwip_receive(int fd, uint8_t *bytes, size_t length,
                          struct arona_lwip_endpoint *peer) {
  if (!peer) {
    ssize_t result = lwip_recv(fd, bytes, length, 0);
    arona_lwip_received(fd, result == 0 && length != 0);
    return result;
  }
  struct sockaddr_in value;
  socklen_t size = sizeof(value);
  ssize_t result = lwip_recvfrom(fd, bytes, length, 0, (struct sockaddr *)&value, &size);
  arona_lwip_received(fd, result == 0 && length != 0);
  if (result >= 0) { endpoint(peer, &value); }
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
  int level = option == 0 ? SOL_SOCKET : IPPROTO_TCP;
  int name = option == 0 ? SO_REUSEADDR : TCP_NODELAY;
  if (set) { return lwip_setsockopt(fd, level, name, value, sizeof(*value)); }
  socklen_t size = sizeof(*value);
  return lwip_getsockopt(fd, level, name, value, &size);
}
