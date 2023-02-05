require 'capybara/dsl'

namespace :html do
  desc 'Generate HTML files for all pages'
  task :all do
    include Capybara::DSL

    Capybara.run_server = false
    Capybara.current_driver = :selenium
    Capybara.app_host = 'http://localhost:3000'

    visit '/users/sign_in'
    fill_in 'user_email', with: 'admin@test.com'
    fill_in 'user_password', with: 'Polo@polo1'
    click_on 'Log in'

    pages = ['/', '/users/sign_in', '/users/password/new','/users/sign_up', '/users/confirmation/new', '/upgrade', '/settings', '/dashboard', '/referral-program/new']

    pages.each do |page|
      visit page
      File.write("public/static#{page.gsub("/", "_")}.html", page.body)
      page.all('link[rel="stylesheet"]').each do |stylesheet|
        css_path = stylesheet[:href]
        css = page.driver.get(css_path).body
        File.write("public/static_#{css_path}", css)
      end
    end
  end
end
