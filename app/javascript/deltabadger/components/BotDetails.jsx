import React from 'react'

export const BotDetails = bot => (
  <div className="db-bots__item db-bot-data">
    <ul className="nav nav-tabs" id="myTab" role="tablist">
      <li className="nav-item">
        <a className="nav-link active" id="stats-tab" data-toggle="tab" href="#stats" role="tab" aria-controls="stats" aria-selected="true">Statistics</a>
      </li>
      <li className="nav-item">
        <a className="nav-link"        id="log-tab"   data-toggle="tab" href="#log"   role="tab" aria-controls="log"   aria-selected="false">Log</a>
      </li>
      <li className="nav-item">
        <a className="nav-link"        id="info-tab"  data-toggle="tab" href="#info"  role="tab" aria-controls="info"  aria-selected="false">Info</a>
      </li>
    </ul>
    <div className="tab-content" id="myTabContent">
      <div className="tab-pane show active" id="stats" role="tabpanel" aria-labelledby="stats-tab">
        <table className="table table-sm db-table db-table--tx">
          <thead>
            <tr>
              <th scope="col">Date</th>
              <th scope="col">Exchange</th>
              <th scope="col">Action</th>
              <th scope="col">Amount(BTC)</th>
              <th scope="col">Price(USD)</th>
            </tr>
          </thead>
          <tbody>
            { bot.transactions.map(t => (
              <tr key={t.id} >
                <th scope="row">{t.created_at}</th>
                <td>{exchangeName}</td>
                <td>{type}</td>
                <td>{t.amount}</td>
                <td>{t.amoun * t.rate}</td>
              </tr>
            ))}
            <tr>
              <th scope="row">11/11/19</th>
              <td>Bitbay</td>
              <td>Buy</td>
              <td>0.00014</td>
              <td>10293.00</td>
            </tr>
            <tr>
              <th scope="row">11/11/19</th>
              <td>Bitbay</td>
              <td>Buy</td>
              <td>0.00014</td>
              <td>10293.00</td>
            </tr>
            <tr>
              <th scope="row">11/11/19</th>
              <td>Bitbay</td>
              <td>Buy</td>
              <td>0.00014</td>
              <td>10293.00</td>
            </tr>
            <tr>
              <th scope="row">11/11/19</th>
              <td>Bitbay</td>
              <td>Buy</td>
              <td>0.00014</td>
              <td>10293.00</td>
            </tr>
            <tr>
              <th scope="row">11/11/19</th>
              <td>Bitbay</td>
              <td>Buy</td>
              <td>0.00014</td>
              <td>10293.00</td>
            </tr>
            <tr>
              <th scope="row">11/11/19</th>
              <td>Bitbay</td>
              <td>Buy</td>
              <td>0.00014</td>
              <td>10293.00</td>
            </tr>
            <tr>
              <th scope="row">11/11/19</th>
              <td>Bitbay</td>
              <td>Buy</td>
              <td>0.00014</td>
              <td>10293.00</td>
            </tr>
            <tr>
              <th scope="row">11/11/19</th>
              <td>Bitbay</td>
              <td>Buy</td>
              <td>0.00014</td>
              <td>10293.00</td>
            </tr>
          </tbody>
        </table>
        <div className="db-bot-info__footer">
          <nav aria-label="Page navigation example">
            <ul className="pagination">
              <li className="page-item"><a className="page-link" href="#">Previous</a></li>
              <li className="page-item"><a className="page-link" href="#">1</a></li>
              <li className="page-item"><a className="page-link" href="#">2</a></li>
              <li className="page-item"><a className="page-link" href="#">3</a></li>
              <li className="page-item"><a className="page-link" href="#">Next</a></li>
            </ul>
          </nav>
          <a href="#" className="btn btn-link btn--export-to-csv">Export to .csv <i className="fas fa-download ml-1"></i></a>
        </div>
      </div>
      <div className="tab-pane" id="log"   role="tabpanel" aria-labelledby="log-tab">Log</div>
      <div className="tab-pane" id="info"  role="tabpanel" aria-labelledby="info-tab">Info</div>
    </div>
  </div>
)
