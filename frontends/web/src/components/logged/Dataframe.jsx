import React, { useState, useEffect } from 'react'
import { Row, Col } from 'react-bootstrap'
import axios from 'axios';
import { CountdownCircleTimer } from 'react-countdown-circle-timer';

function humanTime(timestamp) {
  var date = new Date(timestamp);
  var year = date.getFullYear();
  var month = ("0" + (date.getMonth() + 1)).slice(-2)
  var day = ("0" + date.getDate()).slice(-2)
  var human_time = year + '-' + month + '-' + day; 
  return human_time;
}

const renderTime = ({ remainingTime }) => {
  if (remainingTime === 0) {
    return <div className="timer">0</div>;
  }
  return (
    <div className="timer">
      <div className="value">{remainingTime}</div>
    </div>
  );
};

function Dataframe() {

  const [loading, setLoading] = useState(true);
  const [data, setData] = useState([])

  const [key, setKey] = useState(0);
  const resetClock = async () => {
    console.log('timer ... dataframe...');
    setKey(prevKey => prevKey + 1);
  }

  useEffect(() => {
    const fetchData = async () =>{
      setLoading(true);
      try {
        const BACKEND_URL = process.env.REACT_APP_BACKEND_URL;
        const {data: response} = await axios.get(BACKEND_URL + '/api/last-week/');
        var response_clean = response.last_week
        response_clean = response_clean.trim();
        var response_json = JSON.parse(response_clean);
        setData(response_json);
      } catch (error) {
      }
      setLoading(false);
    }
    const interval = setInterval(() => {
      resetClock();
      fetchData();
    }, 60000);
    return () => clearInterval(interval);
  }, []);

  if (!data) return null;

  const DisplayData=data.map(
    (info)=>{
      var human_time = humanTime(info.Date);
      return(
        <tr key={info.Date}>
          <td>{human_time}</td>
          <td>{info.Open}</td>
          <td>{info.High}</td>
          <td>{info.Low}</td>
          <td>{info.Close}</td>
          <td>{info.Volume}</td>
        </tr>
      )
    }
  )

  return(
        <div>
          <Row xs={2} md={4} lg={6}>
            <Col>
              <div className="timer-wrapper" style={{textAlign: 'right'}}>
                <CountdownCircleTimer
                  key={key}
                  isPlaying
                  duration={60}
                  colors={[["#004777", 0.33], ["#F7B801", 0.33], ["#A30000"]]}
                  onComplete={() => [true, 1000]}
                  size={50}
                  strokeWidth={3}
                >
                  {renderTime}
                </CountdownCircleTimer>
              </div>
            </Col>
            <Col>
              <div style={{ marginLeft: "auto" }}>
                <button onClick={resetClock} style={{ marginLeft: "auto" }}>
                  Reload
                </button>
              </div>
            </Col>
          </Row>
          <table className="table table-striped">
            <thead>
              <tr>
              <th>Date</th>
              <th>Open</th>
              <th>High</th>
              <th>Low</th>
              <th>Close</th>
              <th>Volume</th>
              </tr>
            </thead>
            <tbody>
              {DisplayData}
            </tbody>
          </table>
        </div>
  )
}

export default Dataframe
