import { BrowserRouter as Router, Routes, Route } from "react-router-dom";
import { ToastContainer } from "react-toastify";
import "react-toastify/dist/ReactToastify.css";
import PrivateRoutes from "./utils/PrivateRoutes";
import AuthContext, { AuthProvider } from "./context/AuthContext";
import HomeScreen from "./screens/HomeScreen";
import SignupScreen from "./screens/SignupScreen";
import LoginScreen from "./screens/LoginScreen";
import LoggedHomeScreen from "./screens/LoggedHomeScreen";
import NoPageScreen from "./screens/NoPageScreen";
import PatternDetectorScreen from "./screens/logged/PatternDetectorScreen";
import TradingIntentScreen from "./screens/logged/TradingIntentScreen";

function App() {
  return (
    <>
      <Router>
        <AuthProvider>
          <Routes>
            <Route element={<NoPageScreen />} path="*" />
            <Route element={<HomeScreen />} path="/" exact />
            <Route element={<LoginScreen />} path="/login" />
            <Route element={<SignupScreen />} path="/signup" />
            <Route element={<PrivateRoutes />}>
              <Route element={<LoggedHomeScreen />} path="/dashboard" />
              <Route element={<LoggedHomeScreen />} path="/feed" /> {/* Legacy route */}
              <Route element={<PatternDetectorScreen />} path="/patterns" />
              <Route element={<TradingIntentScreen />} path="/trading-intent/:intentId" />
            </Route>
          </Routes>
          <ToastContainer
            position="bottom-right"
            autoClose={5000}
            hideProgressBar={false}
            newestOnTop={false}
            closeOnClick
            pauseOnFocusLoss
            pauseOnHover
            theme="light"
          />
        </AuthProvider>
      </Router>
    </>
  );
}

export default App;
