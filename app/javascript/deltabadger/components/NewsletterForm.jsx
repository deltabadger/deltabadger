import React, { useState } from 'react'
import API from '../lib/API';

export const NewsletterForm = () => {
  const [email, setEmail] = useState("");
  const [resultInfo, setResultInfo] = useState(undefined);

  const disableSubmit = email == ''
  const successResult = resultInfo && resultInfo[0] == "Success"

  const handleSubmit = (evt) => {
    evt.preventDefault();
    !disableSubmit && API.addSubscriber(email).then(({data}) => {
      setEmail("")
      setResultInfo(["Success"])
    }).catch((data) => {
      setResultInfo(data.response.data.errors)
    })
  }

  return (
    <div className="db-newsletter--form">
      <div className="form-group">
        <input
          id="newsletter-input"
          type="email"
          value={email}
          onChange={e => setEmail(e.target.value)}
          className={`form-control ${resultInfo && resultInfo.map(() => successResult ? ' is-valid' : ' is-invalid')}` } // TODO: fix it to not returning undefined
        />
        <div className="newsletter-form-feedback">
          { resultInfo && resultInfo.map((text, index) => <b key={index} className={`${successResult ? 'text-success--lighter' : 'text-danger--lighter'}` }>{text}.</b>) } <i>.</i>
        </div>
      </div>
      <div className="form-group">
        <div onClick={handleSubmit} className={`btn ${disableSubmit ? 'disabled btn-outline-success' : 'btn-success'}`}>
          <div className="d-block d-sm-none m-0">Send</div>
          <div className="d-none d-sm-block m-0">Keep me in the loop!</div>

        </div>
      </div>
    </div>
  )
}
