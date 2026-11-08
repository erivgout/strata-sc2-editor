// Trunk initializer: reports download progress on the loading screen.
export default function strataInitializer() {
  const loading = () => document.getElementById("loading");
  const status = () => document.getElementById("loading-status");
  const fill = () => document.querySelector("#loading .fill");
  const mb = (n) => (n / (1024 * 1024)).toFixed(1);
  return {
    onStart: () => {
      if (!loading().classList.contains("failed")) status().textContent = "Downloading the editor…";
    },
    onProgress: ({ current, total }) => {
      if (loading().classList.contains("failed")) return;
      if (total > 0) {
        loading().classList.add("measured");
        fill().style.width = `${Math.min(100, (current / total) * 100).toFixed(1)}%`;
        status().textContent = `Downloading the editor… ${mb(current)} of ${mb(total)} MB`;
      } else {
        status().textContent = `Downloading the editor… ${mb(current)} MB`;
      }
    },
    onComplete: () => {
      if (!loading().classList.contains("failed")) status().textContent = "Starting…";
    },
    onSuccess: () => {},
    onFailure: (error) => {
      window.strataLoadingFailed(
        "Strata could not load. Check your connection and reload the page.",
        String(error)
      );
    },
  };
}
