import { useContext, useEffect } from "react";
import { useLocation } from "react-router-dom";
import AuthContext from "../../context/AuthContext";
import RobsonCommandDock from "../logged/RobsonCommandDock";

const isPrivatePath = (pathname) =>
  pathname === "/dashboard" ||
  pathname === "/feed" ||
  pathname === "/patterns" ||
  pathname.startsWith("/trading-intent/");

function AuthenticatedOverlays() {
  const location = useLocation();
  const { authTokens } = useContext(AuthContext);

  const showDock = Boolean(authTokens?.access) && isPrivatePath(location.pathname);

  useEffect(() => {
    document.body.classList.toggle("robson-chat-dock-active", showDock);

    return () => {
      document.body.classList.remove("robson-chat-dock-active");
    };
  }, [showDock]);

  if (!showDock) {
    return null;
  }

  return <RobsonCommandDock />;
}

export default AuthenticatedOverlays;
