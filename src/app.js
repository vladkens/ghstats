const renderMetrics = (canvasId, metrics, uniqueCol, countCol) => {
  const ctx = document.getElementById(canvasId);
  new Chart(ctx, {
    type: 'bar',
    options: {
      responsive: true,
      scales: {
        x: { stacked: true }
      }
    },
    data: {
      labels: metrics.map(x => x.date.split('T')[0]),
      datasets: [
        { label: 'Unique', data: metrics.map(x => x[uniqueCol]) },
        { label: 'Count', data: metrics.map(x => x[countCol]) },
      ]
    },
  });
}

renderMetrics('chart_clones', metrics, 'clones_uniques', 'clones_count');
renderMetrics('chart_views', metrics, 'views_uniques', 'views_count');
