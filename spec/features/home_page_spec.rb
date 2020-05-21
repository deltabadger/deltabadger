require 'rails_helper'

describe 'Opening home page', type: :feature, js: true do
  xit 'opens page' do
    visit '/'
    expect(page).to have_content 'Home page'
  end

  context 'with signed in user' do
    xit 'opens page' do
      sign_in_user

      visit '/'
      expect(page).to have_current_path('/dashboard')
      expect(page).to have_content 'Add bot'
    end
  end
end
