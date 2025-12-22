import { BrowserRouter as Router, Routes, Route } from 'react-router-dom'
import { ToastContainer } from 'react-toastify'
import 'react-toastify/dist/ReactToastify.css'
import PrivateRoutes from './utils/PrivateRoutes'
import AuthContext, { AuthProvider } from './context/AuthContext'
import HomeScreen from './screens/HomeScreen'
import FeaturesScreen from './screens/FeaturesScreen'
import PricingScreen from './screens/PricingScreen'
import AboutUsScreen from './screens/AboutUsScreen'
import OurTeamScreen from './screens/OurTeamScreen'
import CareersScreen from './screens/CareersScreen'
import ContactScreen from './screens/ContactScreen'
import SignupScreen from './screens/SignupScreen'
import LoginScreen from './screens/LoginScreen'
import DownloadScreen from './screens/DownloadScreen'
import HireScreen from './screens/HireScreen'
import DemoScreen from './screens/DemoScreen'
import LoggedHomeScreen from './screens/LoggedHomeScreen'
import NoPageScreen from "./screens/NoPageScreen"

function App() {
  return (
    <>
      <Router>
        <AuthProvider>
          <Routes>
            <Route element={<NoPageScreen/>} path="*"/>
            <Route element={<HomeScreen />} path="/" exact />
            <Route element={<LoginScreen />} path="/login" />
            <Route element={<FeaturesScreen/>} path="/features" />
            <Route element={<PricingScreen/>} path="/pricing" />
            <Route element={<AboutUsScreen/>} path="/about-us"/>
            <Route element={<OurTeamScreen/>} path="/our-team" />
            <Route element={<CareersScreen/>} path="/careers" />
            <Route element={<ContactScreen/>} path="/contact" />
            <Route element={<SignupScreen/>} path="/signup" />
            <Route element={<LoginScreen/>} path="/login" />
            <Route element={<DownloadScreen/>} path="/download" />
            <Route element={<HireScreen/>} path="/hire" />
            <Route element={<DemoScreen/>} path="/demo"/>
            <Route element={<PrivateRoutes />}>
              <Route element={<LoggedHomeScreen />} path="/feed" />
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

export default App
