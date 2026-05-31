(() => {
  // https://stackoverflow.com/a/68140000/3664464
  const mouseLinePlugin = {
    afterDraw: chart => {
      if (!chart.tooltip?._active?.length) return;
      let x = chart.tooltip._active[0].element.x;
      let yAxis = chart.scales.y;
      let ctx = chart.ctx;
      ctx.save();
      ctx.beginPath();
      ctx.moveTo(x, yAxis.top);
      ctx.lineTo(x, yAxis.bottom);
      ctx.lineWidth = 1;
      ctx.strokeStyle = 'rgba(0, 0, 255, 0.4)';
      ctx.stroke();
      ctx.restore();
    },
  };

  const parseMetricDate = value => {
    const [year, month, day] = value.split('T')[0].split('-').map(Number);
    return new Date(Date.UTC(year, month - 1, day));
  };

  const formatDate = date =>
    `${date.getUTCFullYear()}-${String(date.getUTCMonth() + 1).padStart(2, '0')}-${String(date.getUTCDate()).padStart(2, '0')}`;

  const getWeekStart = date => {
    const start = new Date(date);
    const day = start.getUTCDay();
    const diff = day === 0 ? -6 : 1 - day;
    start.setUTCDate(start.getUTCDate() + diff);
    return start;
  };

  const getBucketKind = metrics => {
    if (metrics.length <= 1) return 'day';
    const first = parseMetricDate(metrics[0].date);
    const last = parseMetricDate(metrics.at(-1).date);
    const rangeDays = Math.ceil((last - first) / 86400000);

    if (rangeDays <= 90) return 'day';
    if (rangeDays <= 365) return 'week';
    return 'month';
  };

  const groupMetrics = (metrics, uniqueCol, countCol) => {
    const bucketKind = getBucketKind(metrics);
    const buckets = new Map();

    for (const metric of metrics) {
      const date = parseMetricDate(metric.date);
      let bucketStart;
      let label;

      if (bucketKind === 'month') {
        bucketStart = new Date(Date.UTC(date.getUTCFullYear(), date.getUTCMonth(), 1));
        label = `${bucketStart.getUTCFullYear()}-${String(bucketStart.getUTCMonth() + 1).padStart(2, '0')}`;
      } else if (bucketKind === 'week') {
        bucketStart = getWeekStart(date);
        label = formatDate(bucketStart);
      } else {
        bucketStart = date;
        label = formatDate(bucketStart);
      }

      const key = formatDate(bucketStart);
      const current = buckets.get(key) ?? {
        x: bucketStart,
        label,
        title: label,
        unique: 0,
        count: 0,
      };

      current.unique += metric[uniqueCol];
      current.count += metric[countCol];
      buckets.set(key, current);
    }

    const items = Array.from(buckets.values()).sort((a, b) => a.x - b.x);
    if (bucketKind === 'week') {
      for (const item of items) {
        const bucketEnd = new Date(item.x);
        bucketEnd.setUTCDate(bucketEnd.getUTCDate() + 6);
        item.title = `${formatDate(item.x)} to ${formatDate(bucketEnd)}`;
      }
    }

    return { bucketKind, items };
  };

  window.renderMetrics = (canvasId, metrics, uniqueCol, countCol) => {
    const ctx = document.getElementById(canvasId);
    if (!ctx) return;

    const grouped = groupMetrics(metrics, uniqueCol, countCol);
    new Chart(ctx, {
      type: 'bar',
      data: {
        labels: grouped.items.map(x => x.x),
        datasets: [
          { label: 'Unique', data: grouped.items.map(x => x.unique), borderWidth: 0, borderRadius: 4 },
          { label: 'Count', data: grouped.items.map(x => x.count), borderWidth: 0, borderRadius: 4 },
        ],
      },
      options: {
        responsive: true,
        interaction: { mode: 'index' },
        scales: {
          x: { stacked: true, type: 'time', time: { tooltipFormat: 'yyyy-MM-dd' } },
          y: { beginAtZero: true },
        },
        plugins: {
          legend: { display: false },
          // title: { display: true, text: uniqueCol.split('_')[0].toUpperCase() }
          tooltip: {
            intersect: false,
            callbacks: {
              title: items => grouped.items[items[0].dataIndex]?.title ?? '',
            },
          },
        },
      },
      plugins: [mouseLinePlugin],
    });
  };

  window.renderStars = (canvasId, stars) => {
    const ctx = document.getElementById(canvasId);
    if (!ctx) return;

    new Chart(ctx, {
      type: 'line',
      data: {
        labels: stars.map(x => x.date.split('T')[0]),
        datasets: [{ label: '', data: stars.map(x => x.stars), pointStyle: false, tension: 0.0 }],
      },
      options: {
        responsive: true,
        maintainAspectRatio: false,
        interaction: { mode: 'index' },
        scales: {
          x: { stacked: true, type: 'time', time: { tooltipFormat: 'yyyy-MM-dd' } },
          y: { beginAtZero: true },
        },
        plugins: {
          legend: { display: false },
          title: { display: false, text: 'Stars', font: { size: 20 }, align: 'start' },
          tooltip: { intersect: false },
        },
      },
      plugins: [mouseLinePlugin],
    });
  };
})();
