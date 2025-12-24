import { Outlet, Navigate, useLocation } from 'react-router-dom'
import { useContext } from 'react'
import AuthContext from '../context/AuthContext'

const PrivateRoutes = () => {
    let { user } = useContext(AuthContext)
    const location = useLocation()
    
    // Check if we're in demo mode via query parameters
    const searchParams = new URLSearchParams(location.search)
    const isDemoMode = searchParams.get('demo') === 'true'
    
    return (
        user || isDemoMode ? <Outlet /> : <Navigate to="/login" />
    )
}

export default PrivateRoutes