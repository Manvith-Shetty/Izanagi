import { useEffect } from "react";
import { Route, Routes, useLocation, Link } from "react-router-dom";
import Landing from "./pages/Landing";
import Dashboard from "./pages/Dashboard";
import Approve from "./pages/Approve";
import Sellers from "./pages/Sellers";
import { Header } from "./components/Chrome";

/** Scroll to the top on a new page, or to the #section a link names. */
function ScrollManager() {
  const { pathname, hash } = useLocation();
  useEffect(() => {
    if (hash) {
      requestAnimationFrame(() => document.getElementById(hash.slice(1))?.scrollIntoView({ behavior: "smooth" }));
    } else {
      window.scrollTo(0, 0);
    }
  }, [pathname, hash]);
  return null;
}

function NotFound() {
  return (
    <div className="ground grain min-h-dvh">
      <Header />
      <main className="mx-auto max-w-[76rem] px-r4 py-r7 md:px-r5">
        <h1 className="display text-[3rem]">Nothing at this address</h1>
        <p className="mt-r3 text-sumi-soft">Check the link, or start from the dashboard.</p>
        <Link to="/dashboard" className="btn btn-primary mt-r5">
          Open dashboard
        </Link>
      </main>
    </div>
  );
}

export default function App() {
  return (
    <>
      <ScrollManager />
      <Routes>
        <Route path="/" element={<Landing />} />
        <Route path="/dashboard" element={<Dashboard />} />
        <Route path="/approve/:id" element={<Approve />} />
        <Route path="/sellers" element={<Sellers />} />
        <Route path="*" element={<NotFound />} />
      </Routes>
    </>
  );
}
