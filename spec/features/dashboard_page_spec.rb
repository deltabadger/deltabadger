require 'rails_helper'

describe 'Opening home page', type: :feature do
  it 'signs me in' do
    user = User.create(
      email: 'test@test.com',
      password: 'password',
      password_confirmation: 'password',
      confirmed_at: Time.now
    )
    sign_in user

    visit '/'
    expect(page).to have_current_path('/dashboard')
    expect(page).to have_content 'Dashboard'
    expect(page).to have_http_status(200)
  end
end
