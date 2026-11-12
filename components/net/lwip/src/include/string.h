// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

#ifndef ARONA_LWIP_STRING_H
#define ARONA_LWIP_STRING_H
#include <stddef.h>
void *memcpy(void *restrict dest, const void *restrict source, size_t count);
void *memmove(void *dest, const void *source, size_t count);
void *memset(void *dest, int value, size_t count);
int memcmp(const void *first, const void *second, size_t count);
size_t arona_lwip_strlen(const char *text);
int arona_lwip_strcmp(const char *first, const char *second);
int arona_lwip_strncmp(const char *first, const char *second, size_t count);
char *arona_lwip_strncpy(char *restrict dest, const char *restrict source, size_t count);
#define strlen arona_lwip_strlen
#define strcmp arona_lwip_strcmp
#define strncmp arona_lwip_strncmp
#define strncpy arona_lwip_strncpy
#endif
