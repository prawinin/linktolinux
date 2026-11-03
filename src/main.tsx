import React from "react";
import ReactDOM from "react-dom/client";
import App from "./CompanionDashboard";
import "./styles.css";

const container = document.getElementById("root");
if (!container) {
  throw new Error("LinkToLinux root container is missing");
}

ReactDOM.createRoot(container).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
