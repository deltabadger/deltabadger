require 'rails_helper'

describe 'Opening dashboard', type: :feature, js: true do
  context 'with guest user' do
    xit 'redirects to sign in page' do
      visit '/dashboard'
      expect(page).to have_current_path('/users/sign_in')
    end
  end

  context 'with signed in user' do
    xit 'opens page' do
      sign_in_user

      visit '/dashboard'
      expect(page).to have_current_path('/dashboard')
    end
  end
end
