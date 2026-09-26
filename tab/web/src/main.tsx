import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { BrowserRouter, Route, Routes } from "react-router-dom";
import "./styles.css";
import Landing from "./pages/Landing";
import Start from "./pages/Start";
import Dashboard from "./pages/Dashboard";
import Approve from "./pages/Approve";

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <BrowserRouter>
      <Routes>
        <Route path="/" element={<Landing />} />
        <Route path="/start" element={<Start />} />
        <Route path="/app" element={<Dashboard />} />
        <Route path="/approve/:id" element={<Approve />} />
        <Route path="*" element={<Landing />} />
      </Routes>
    </BrowserRouter>
  </StrictMode>,
);
