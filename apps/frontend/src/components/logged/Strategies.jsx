import React, { useState, useEffect, useContext, useMemo } from 'react'
import AuthContext from '../../context/AuthContext'
import { TradeHttp } from '../../adapters/http/TradeHttp'

function Strategies() {

  let [strategies, setStrategies] = useState([])
  let { authTokens, logoutUser } = useContext(AuthContext)

  const service = useMemo(() => new TradeHttp({
    baseUrl: import.meta.env.VITE_API_BASE_URL,
    getAuthToken: () => authTokens?.access || null,
  }), [authTokens])
  useEffect(() => {
    getStrategies()
  }, [])

  let getStrategies = async () => {
    try {
      const data = await service.getStrategies()
      setStrategies(data)
    } catch (e) {
      console.log('Failed to load strategies', e)
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
