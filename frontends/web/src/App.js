import { BrowserRouter, Routes, Route } from 'react-router-dom'
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
    <BrowserRouter>
      <Routes>
        <Route exact path="/" element={<HomeScreen />} />
        <Route path="/features" element={<FeaturesScreen/>} />
        <Route path="/pricing" element={<PricingScreen/>} />
        <Route path="/about-us" element={<AboutUsScreen/>} />
        <Route path="/our-team" element={<OurTeamScreen/>} />
        <Route path="/careers" element={<CareersScreen/>} />
        <Route path="/contact" element={<ContactScreen/>} />
        <Route path="/signup" element={<SignupScreen/>} />
        <Route path="/login" element={<LoginScreen/>} />
        <Route path="/download" element={<DownloadScreen/>} />
        <Route path="/hire" element={<HireScreen/>} />
        <Route path="/demo" element={<DemoScreen/>} />
        <Route path="/feed" element={<LoggedHomeScreen/>} />
        <Route path="*" element={<NoPageScreen/>} />
      </Routes>
    </BrowserRouter>
    </>
  );
}

export default App
