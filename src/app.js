const renderMetrics = (canvasId, metrics, uniqueCol, countCol) => {
  const ctx = document.getElementById(canvasId);
  new Chart(ctx, {
    type: 'bar',
    options: {
      responsive: true,
      scales: {
        x: { stacked: true },
        y: { beginAtZero: true },
      },
      plugins: {
        legend: { display: false },
        // title: { display: true, text: uniqueCol.split('_')[0].toUpperCase() }
      },
    },
    data: {
      labels: metrics.map(x => x.date.split('T')[0]),
      datasets: [
        { label: 'Unique', data: metrics.map(x => x[uniqueCol]), borderWidth: 0, borderRadius: 4 },
        { label: 'Count', data: metrics.map(x => x[countCol]), borderWidth: 0, borderRadius: 4 },
      ],
    },
  });
};

const renderStars = (canvasId, stars) => {
  const ctx = document.getElementById(canvasId);
  new Chart(ctx, {
    type: 'line',
    options: {
      responsive: true,
      scales: {
        x: { stacked: true },
        y: { beginAtZero: true },
      },
      plugins: {
        legend: { display: false },
        // title: { display: false, text: 'Stars' },
      },
    },
    data: {
      labels: stars.map(x => x.date.split('T')[0]),
      datasets: [{ label: '', data: stars.map(x => x.stars) }],
    },
  });
};
