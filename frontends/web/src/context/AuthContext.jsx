import { createContext, useState, useEffect } from 'react'
import jwt_decode from 'jwt-decode'
import { useNavigate } from 'react-router-dom'

const AuthContext = createContext()
export default AuthContext;

function ErrorMessage({ error }) {
  return (
    <div style={{ color: "red" }}>
      <p>Error: {error}</p>
    </div>
  );
}

export const AuthProvider = ({ children }) => {

  let [authTokens, setAuthTokens] = useState(() => localStorage.getItem('authTokens') ? JSON.parse(localStorage.getItem('authTokens')) : null)
  let [user, setUser] = useState(() => localStorage.getItem('authTokens') ? jwt_decode(localStorage.getItem('authTokens')) : null)
  let [loading, setLoading] = useState(true)
  let [error, setError] = useState(null);

  const navigate = useNavigate()

  let loginUser = async (e) => {
    e.preventDefault()
    try {
      let response = await fetch('http://127.0.0.1:8403/api/token/', {
        method: 'POST',
        headers: {
          'Content-Type': 'application/json'
        },
        body: JSON.stringify({ 'username': e.target.username.value, 'password': e.target.password.value })
      })
      let data = await response.json()
      if (response.status !== 200) {
        throw new Error('Something went wrong!')
      }
      if (response.status === 200) {
        setAuthTokens(data)
        setUser(jwt_decode(data.access))
        localStorage.setItem('authTokens', JSON.stringify(data))
        navigate('/feed')
      } else {
        alert('Something went wrong!')
      }
    } catch (err) {
      setError(err.message);
    }
  }

  let logoutUser = () => {
    setAuthTokens(null)
    setUser(null)
    localStorage.removeItem('authTokens')
    navigate('/login')
  }

  let updateToken = async () => {
    try {
      console.log('Updated token.');
      let response = await fetch('http://127.0.0.1:8403/api/token/refresh/', {
        method: 'POST',
        headers: {
          'Content-Type': 'application/json'
        },
        body: JSON.stringify({ 'refresh': authTokens?.refresh })
      })
      let data = await response.json()
      if (response.status !== 200) {
        throw new Error('Something went wrong!')
      }
      if (response.status === 200) {
        setAuthTokens(data)
        setUser(jwt_decode(data.access))
        localStorage.setItem('authTokens', JSON.stringify(data))
      } else {
        logoutUser()
      }
    } catch (err) {
      setError(err.message);
    }

    if (loading) {
      setLoading(false)
    }
  }

  let contextData = {
    user: user,
    authTokens: authTokens,
    loginUser: loginUser,
    logoutUser: logoutUser
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
      {error && <ErrorMessage error={error} />}
      {children}
    </AuthContext.Provider>
  )
}