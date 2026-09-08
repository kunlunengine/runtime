(() => {
  const equal = (actual, expected) => {
    if (actual !== expected) throw new Error(`Temporal: expected ${expected}, got ${actual}`);
  };
  equal(Temporal.PlainDate.from('2024-02-28').add({ days: 1 }).toString(), '2024-02-29');
  equal(Temporal.PlainDateTime.from('2024-02-29T23:30').add({ hours: 1 }).toString(), '2024-03-01T00:30:00');
  equal(Temporal.PlainTime.from('23:30').add({ hours: 1 }).toString(), '00:30:00');
  equal(Temporal.PlainYearMonth.from('2024-02').daysInMonth, 29);
  equal(Temporal.PlainMonthDay.from('--02-29').toString(), '02-29');
  equal(Temporal.Duration.from('PT90M').total({ unit: 'hours' }), 1.5);
  equal(Temporal.Instant.from('1970-01-01T00:00Z').add({ nanoseconds: 1 }).epochNanoseconds, 1n);
  const dst = Temporal.ZonedDateTime.from('2024-03-10T01:30-05:00[America/New_York]').add({ hours: 1 });
  equal(dst.hour, 3);
  equal(dst.offset, '-04:00');
  equal(typeof Temporal.Now.instant().epochNanoseconds, 'bigint');
  let invalid = false;
  try { Temporal.PlainDate.from('2024-02-30', { overflow: 'reject' }); } catch (error) { invalid = error instanceof RangeError; }
  equal(invalid, true);
  return 'temporal-ok';
})()
