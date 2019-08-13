require 'rails_helper'

describe 'Opening home page', type: :feature do
  it 'opens page' do
    visit '/'
    expect(page).to have_content 'Home page'
    expect(page).to have_http_status(200)
  end

  context 'with signed in user' do
    it 'opens page' do
      sign_in_user

      visit '/'
      expect(page).to have_current_path('/dashboard')
      expect(page).to have_content 'Dashboard'
      expect(page).to have_http_status(200)
    end
  end
end
