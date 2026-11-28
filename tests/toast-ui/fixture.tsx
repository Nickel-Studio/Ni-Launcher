import { StrictMode, useLayoutEffect, useState } from "react";
import { createRoot } from "react-dom/client";
import { ToastHost } from "../../src/components/ui/ToastHost";
import { dismissToast, showToast, useToasts } from "../../src/lib/toast";
import "../../src/styles/tokens.css";
import "../../src/styles/global.css";

const bridge = window as any;
bridge.__toast = { showToast, dismissToast, messages: [], clicks: 0 };
function Fixture() {
  const messages = useToasts();
  const [mounted, setMounted] = useState(true);
  useLayoutEffect(() => {
    bridge.__toast.messages = messages;
    bridge.__toast.setMounted = setMounted;
  }, [messages]);
  return (
    <>
      <button id="under-toast" style={{ position: "fixed", left: 0, bottom: 20, width: 420, height: 200 }} onClick={() => bridge.__toast.clicks++}>
        Underlying control
      </button>
      {mounted && <ToastHost />}
    </>
  );
}
createRoot(document.getElementById("root")!).render(<StrictMode><Fixture /></StrictMode>);
