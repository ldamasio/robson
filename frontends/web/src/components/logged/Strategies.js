import React, { useState, useEffect, useContext } from 'react'
import AuthContext from 'context/AuthContext'

function Strategies() {

  let [strategies, setStrategies] = useState([])
  let { authTokens, logoutUser } = useContext(AuthContext)
  useEffect(() => {
    getStrategies()
  }, [])

  let getStrategies = async () => {
    console.log('getStrategies().');
    let response = await fetch('http://127.0.0.1:8000/api/strategies/', {
      method: 'GET',
      headers: {
        'Content-Type': 'application/json',
        'Authorization': 'Bearer ' + String(authTokens.access)
      }
    })
    let data = await response.json()

    if (response.status === 200) {
      console.log(data)
      setStrategies(data)
    } else {
      console.log("not 200 strat")
    }
  }


  //   if (loading) {
  //     setLoading(false)
  //   }
  // }

  return (
    <div>
      Strategies
      <ul>
        {strategies.map(strategy => (
          <li key={strategy.id}>{strategy.name}</li>
        ))}
      </ul>
    </div>
  )
}

export default Strategies
