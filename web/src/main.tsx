import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { BrowserRouter, Navigate, Outlet, Route, Routes } from "react-router-dom";
import "./index.css";
import { Layout } from "./components/Layout";
import { ApplicationsPage } from "./pages/ApplicationsPage";
import { JobsPage } from "./pages/JobsPage";
import { NewApplicationPage } from "./pages/NewApplicationPage";
import { ResumePage } from "./pages/ResumePage";
import { SettingsPage } from "./pages/SettingsPage";
import { LoginPage } from "./pages/LoginPage";
import { api } from "./api";
import { useEffect, useState } from "react";

function RequireLogin() {
  const [loggedIn, setLoggedIn] = useState<boolean | null>(null);
  useEffect(() => {
    api.currentUser().then(() => setLoggedIn(true), () => setLoggedIn(false));
  }, []);
  if (loggedIn === null) return <div className="p-8 text-sm text-zinc-500">Checking your session…</div>;
  return loggedIn ? <Outlet /> : <Navigate to="/login" replace />;
}

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <BrowserRouter>
      <Routes>
        <Route path="login" element={<LoginPage />} />
        <Route element={<RequireLogin />}>
          <Route element={<Layout />}>
            <Route index element={<ApplicationsPage />} />
            <Route path="jobs" element={<JobsPage />} />
            <Route path="new" element={<NewApplicationPage />} />
            <Route path="resume" element={<ResumePage />} />
            <Route path="settings" element={<SettingsPage />} />
          </Route>
        </Route>
      </Routes>
    </BrowserRouter>
  </StrictMode>,
);
