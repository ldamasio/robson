

import { createContext, useState, useEffect } from 'react'
import jwt_decode from 'jwt-decode'
import { useNavigate } from 'react-router-dom'

const AuthContext = createContext()
export default AuthContext;

export const AuthProvider = ({ children }) => {

  let [authTokens, setAuthTokens] = useState(() => localStorage.getItem('authTokens') ? JSON.parse(localStorage.getItem('authTokens')) : null)
  let [user, setUser] = useState(() => localStorage.getItem('authTokens') ? jwt_decode(localStorage.getItem('authTokens')) : null)
  let [loading, setLoading] = useState(true)
  let [error, setError] = useState(null);

  const navigate = useNavigate()

  let loginUser = async (e) => {
    e.preventDefault()
    setError(null) // Clear previous errors

    // Defensive check for API Base URL
    const baseUrl = import.meta.env.VITE_API_BASE_URL;
    if (!baseUrl || baseUrl === 'undefined' || baseUrl === '') {
      setError("System Error: API URL is not configured. Please check .env file.");
      return;
    }

    try {
      let response = await fetch(`${baseUrl}/api/token/`, {
        method: 'POST',
        headers: {
          'Content-Type': 'application/json'
        },
        body: JSON.stringify({ 'username': e.target.username.value, 'password': e.target.password.value })
      })

      let data;
      // Robust JSON parsing
      const contentType = response.headers.get("content-type");
      if (contentType && contentType.indexOf("application/json") !== -1) {
        data = await response.json();
      } else {
        // Handle non-JSON response (likely 404 html or empty)
        if (!response.ok) {
          throw new Error(`Server connection failed (${response.status} ${response.statusText})`);
        }
      }

      if (response.status === 200) {
        setAuthTokens(data)
        setUser(jwt_decode(data.access))
        localStorage.setItem('authTokens', JSON.stringify(data))
        navigate('/feed')
      } else {
        // API returned an error (e.g. 401 Unauthorized)
        setError(data?.detail || 'Invalid username or password');
      }
    } catch (err) {
      console.error("Login failed:", err)
      setError(err.message || 'Network request failed');
    }
  }

  useEffect(() => {
    if (import.meta.env.VITE_API_BASE_URL) {
      console.log('API_BASE_URL:', import.meta.env.VITE_API_BASE_URL)
    }
  }, [])

  let logoutUser = () => {
    setAuthTokens(null)
    setUser(null)
    localStorage.removeItem('authTokens')
    navigate('/login')
  }

  let updateToken = async () => {
    try {
      console.log('Updated token.');
      let response = await fetch(`${import.meta.env.VITE_API_BASE_URL}/api/token/refresh/`, {
        method: 'POST',
        headers: {
          'Content-Type': 'application/json'
        },
        body: JSON.stringify({ 'refresh': authTokens?.refresh })
      })

      let data = await response.json()

      if (response.status === 200) {
        setAuthTokens(data)
        setUser(jwt_decode(data.access))
        localStorage.setItem('authTokens', JSON.stringify(data))
      } else {
        logoutUser()
      }
    } catch (err) {
      console.error("Token refresh failed", err)
      logoutUser()
    }

    if (loading) {
      setLoading(false)
    }
  }

  let contextData = {
    user: user,
    authTokens: authTokens,
    loginUser: loginUser,
    logoutUser: logoutUser,
    error: error // Expose error state
  }

  useEffect(() => {

    if (loading) {
      updateToken()
    }

    let fourMinutes = 1000 * 60 * 4
    let interval = setInterval(() => {
      if (authTokens) {
        updateToken()
      }
    }, fourMinutes)
    return () => clearInterval(interval)

  }, [authTokens, loading])

  return (
    <AuthContext.Provider value={contextData}>
      {children}
    </AuthContext.Provider>
  )
}

