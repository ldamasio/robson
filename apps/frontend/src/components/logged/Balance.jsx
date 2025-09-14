import React, { useState, useEffect } from 'react'
import { Card } from 'react-bootstrap'
import axios from 'axios';

function Balance() {

  const [loading, setLoading] = useState(true);
  const [data, setData] = useState([])

  useEffect(() => {
    const fetchData = async () =>{
      setLoading(true);
      try {
        const BACKEND_URL = import.meta.env.VITE_API_BASE_URL;
        const { data: response } = await axios.get(`${BACKEND_URL}/api/balance/`);
        setData(response);
        console.log(response)
      } catch (error) {
        console.error(error.message);
      }
      setLoading(false);
    }
    const interval = setInterval(() => {
      fetchData();
    }, 500000);
    return () => clearInterval(interval);
  }, []);

 if (!data) return null;

  return (
        <div>
          {loading && 
            <div>
              <Card.Body>
                Spot: {data.spot}
                <br />
                Isolated Margin: {data.isolated_margin}
              </Card.Body>
            </div>}
          {!loading && (
            <div>
              <Card.Body>
                Spot: {data.spot}
                <br />
                Isolated Margin: {data.isolated_margin}
              </Card.Body>
            </div>
          )}
        </div>
   )

}

export default Balance
