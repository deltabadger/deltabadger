require 'rails_helper'

describe 'Opening home page', type: :feature do
  it 'signs me in' do
    visit '/'
    expect(page).to have_content 'Home page'
    expect(page).to have_http_status(200)
  end
end
