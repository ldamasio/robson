import { Container, Tab, Tabs } from 'react-bootstrap'
import Header from '../components/common/Header'
import Footer from '../components/common/Footer'
import Patrimony from '../components/logged/Patrimony'
import Balance from '../components/logged/Balance'
import ActualPrice from '../components/logged/ActualPrice'
import Position from '../components/logged/Position'
import Trend from '../components/logged/Trend'
import Strategies from '../components/logged/Strategies'
import Risk from '../components/logged/Risk'
import Volume from '../components/logged/Volume'
import Chart from '../components/logged/Chart'
import Dataframe from '../components/logged/Dataframe'

function LoggedHomeScreen() {
  return (
    <div>
      <Header />
      <main className="py-5">
        <Container>
          <p>
            <small>You are running this application in <b>{process.env.NODE_ENV}</b> mode.</small>
            <br />
            <small>BACKEND_URL is <b>{process.env.REACT_APP_BACKEND_URL}</b></small>
          </p>
          <Tabs defaultActiveKey="1">
            <Tab eventKey="1" title="Control Panel">
              <h1>Patrimony</h1>
              <Patrimony />
              <h1>Balance</h1>
              <Balance />
              <h1>Actual Price</h1>
              <ActualPrice />
              <h1>Position</h1>
              <Position />
              <h1>Trend Now</h1>
              <Trend />
              <h1>Best Strategies</h1>
              <Strategies />
              <h1>Risk Indicator</h1>
              <Risk />
              <h1>Volume BTC USDT Last 24h</h1>
              <Volume />
              <h1>BTC USDT 4 Hour Chart</h1>
              <Chart />
              <h1>BTC USDT Last Week Dataframe</h1>
              <Dataframe />
            </Tab>
            <Tab eventKey="2" title="Indicators">
              Conteudo 2
            </Tab>
            <Tab eventKey="3" title="Operations">
              Conteudo 3
            </Tab>
          </Tabs>
        </Container> 
      </main>
      <Footer />
    </div>
  );
}

export default LoggedHomeScreen;
