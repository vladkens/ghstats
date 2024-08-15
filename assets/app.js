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

const renderMetrics = (canvasId, metrics, uniqueCol, countCol) => {
  const ctx = document.getElementById(canvasId);
  new Chart(ctx, {
    type: 'bar',
    data: {
      labels: metrics.map(x => x.date.split('T')[0]),
      datasets: [
        { label: 'Unique', data: metrics.map(x => x[uniqueCol]), borderWidth: 0, borderRadius: 4 },
        { label: 'Count', data: metrics.map(x => x[countCol]), borderWidth: 0, borderRadius: 4 },
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
        tooltip: { intersect: false },
      },
    },
    plugins: [mouseLinePlugin],
  });
};

const renderStars = (canvasId, stars) => {
  const ctx = document.getElementById(canvasId);
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
