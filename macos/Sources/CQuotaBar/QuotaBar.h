#ifndef QUOTABAR_H
#define QUOTABAR_H
#include <stdint.h>
typedef struct QBScanner QBScanner;
typedef struct QBCancel QBCancel;

QBScanner *qb_scanner_new(void);
void qb_scanner_free(QBScanner *);
void qb_scanner_release_cache(const QBScanner *);
QBCancel *qb_cancel_new(void);
void qb_cancel_free(QBCancel *);
void qb_cancel_set(const QBCancel *);
/* Releases no arguments. Success returns 0 and an owned UTF-8 JSON string.
   Cancellation returns 1. Other failures return -1. */
int32_t qb_scanner_scan(const QBScanner *, const QBCancel *, const char *request, char **output);
void qb_string_free(char *);
#endif
