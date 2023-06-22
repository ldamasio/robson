import React, { useState, useEffect } from 'react'
import { Card } from 'react-bootstrap'
import axios from 'axios';

function Patrimony() {

  const [loading, setLoading] = useState(true);
  const [data, setData] = useState([])

  useEffect(() => {
    const resetClock = async () => {
      console.log('timer ...');
    }
    const fetchData = async () => {
      setLoading(true);
      try {
        const BACKEND_URL = process.env.REACT_APP_BACKEND_URL;
        const {data: response} = await axios.get(BACKEND_URL + '/api/patrimony/');
        setData(response);
      } catch (error) {
        console.error(error.message);
      }
      setLoading(false);
    }
    const interval = setInterval(() => {
      fetchData();
      resetClock();
    }, 300000);
    return () => clearInterval(interval);
  }, []);

 if (!data) return null;

  return (
        <div>
          {loading && 
            <div>
              <Card.Body>
                {data.patrimony}
              </Card.Body>
            </div>}
          {!loading && (
            <div>
              <Card.Body>
                {data.patrimony}
              </Card.Body>
            </div>
          )}
        </div>
   )

}

export default Patrimony
