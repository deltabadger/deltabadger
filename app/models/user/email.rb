class User::Email
  ADDRESS_PATTERN = '^[a-zA-Z0-9._%+\-]+@[a-zA-Z0-9.\-]+\.[a-zA-Z]{2,}$'.freeze

  def self.google_email?(email)
    email.end_with?('@gmail.com') || email.end_with?('@googlemail.com')
  end

  def self.google_email_username(email)
    return unless google_email?(email)

    email.split('@').first.split('+').first.downcase
  end

  def self.real_email(email)
    if google_email?(email)
      google_username = google_email_username(email)
      User.find_by(email: "#{google_username}@gmail.com")&.email ||
        User.find_by(email: "#{google_username}@googlemail.com")&.email ||
        User.where('email LIKE ?', "#{google_username}+%@gmail.com").pluck(:email).first ||
        User.where('email LIKE ?', "#{google_username}+%@googlemail.com").pluck(:email).first
    else
      email
    end
  end

  def self.google_email_exists?(email, exclude_emails: [])
    return false unless google_email?(email)

    google_username = google_email_username(email)
    google_usernames = User.where('email LIKE ? OR email LIKE ?', '%@gmail.com', '%@googlemail.com')
                           .pluck(:email)
                           .reject { |e| exclude_emails.include?(e) }
                           .map { |e| User::Email.google_email_username(e) }
    google_usernames.include?(google_username)
  end
end
