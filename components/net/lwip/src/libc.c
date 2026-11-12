// SPDX-License-Identifier: MPL-2.0 OR GPL-3.0-or-later

#include <string.h>
#include <stdlib.h>
#include <limits.h>

size_t strlen(const char *text) {
  const char *end = text;
  while (*end) { ++end; }
  return (size_t)(end - text);
}

int strncmp(const char *first, const char *second, size_t count) {
  for (size_t i = 0; i < count; ++i) {
    unsigned char a = (unsigned char)first[i];
    unsigned char b = (unsigned char)second[i];
    if (a != b || !a) { return (int)a - (int)b; }
  }
  return 0;
}

int strcmp(const char *first, const char *second) {
  while (*first && *first == *second) { ++first; ++second; }
  return (int)(unsigned char)*first - (int)(unsigned char)*second;
}

char *strncpy(char *restrict dest, const char *restrict source, size_t count) {
  size_t i = 0;
  while (i < count && source[i]) { dest[i] = source[i]; ++i; }
  while (i < count) { dest[i++] = 0; }
  return dest;
}

int atoi(const char *text) {
  while (*text == ' ' || (*text >= '\t' && *text <= '\r')) { ++text; }
  int negative = *text == '-';
  if (*text == '-' || *text == '+') { ++text; }
  unsigned int value = 0;
  while (*text >= '0' && *text <= '9') {
    unsigned int digit = (unsigned int)(*text++ - '0');
    if (value > ((unsigned int)INT_MAX - digit) / 10) { return 0; }
    value = value * 10 + digit;
  }
  return negative ? -(int)value : (int)value;
}
