# DaemonEye Documentation

This directory contains the comprehensive documentation for DaemonEye, built using [mdbook](https://rust-lang.github.io/mdBook/).

## Structure

The documentation is organized into the following sections:

- **Introduction**: Overview and getting started
- **Architecture**: System architecture and design principles
- **Technical**: Technical specifications and implementation details
- **User Guides**: Comprehensive user and operator guides
- **API Reference**: Complete API documentation
- **Deployment**: Installation and deployment guides
- **Security**: Security considerations and best practices
- **Testing**: Testing strategies and guidelines
- **Contributing**: Contribution guidelines and development setup

## Building the Documentation

### Prerequisites

Install mdbook:

```bash
cargo install mdbook
```

### Build Commands

```bash
# Build the documentation
mdbook build

# Serve the documentation locally
mdbook serve

# Serve on a specific port
mdbook serve --port 3000

# Watch for changes and rebuild
mdbook watch
```

### Output

The built documentation will be available in the `book/` directory and can be served by any web server.

## Development

### Adding New Content

1. Add new markdown files to the `src/` directory
2. Update `src/SUMMARY.md` to include the new content
3. Build and test the documentation

### Configuration

The documentation is configured via `book.toml`. Key settings:

- **Theme**: Navy theme with dark mode support
- **Git Integration**: Links to GitHub repository
- **Folding**: Enable section folding
- **Build Directory**: `book/` directory

### Preprocessors

The documentation uses several mdbook preprocessors:

- **mermaid**: For diagrams and flowcharts
- **toc**: For table of contents generation
- **open-on-gh**: For GitHub integration
- **tabs**: For tabbed content
- **yml-header**: For YAML front matter

## Content Guidelines

### Writing Style

- Use clear, concise language
- Include code examples where appropriate
- Follow markdown best practices
- Use consistent formatting

### Code Examples

- Use syntax highlighting for code blocks
- Include complete, runnable examples
- Test all code examples
- Use appropriate language tags

### Images and Diagrams

- Use Mermaid for diagrams when possible
- Optimize images for web display
- Include alt text for accessibility
- Use relative paths for local images

## Deployment

### GitHub Pages

The documentation can be deployed to GitHub Pages:

1. Enable GitHub Pages in repository settings
2. Set source to GitHub Actions
3. Use the provided GitHub Actions workflow

### Other Platforms

The built documentation can be deployed to any static hosting platform:

- **Netlify**: Drag and drop the `book/` directory
- **Vercel**: Connect the repository and set build command
- **AWS S3**: Upload the `book/` directory contents
- **Azure Static Web Apps**: Deploy the `book/` directory

## Contributing

See the [Contributing Guide](../CONTRIBUTING.md) for information on contributing to the documentation.

## License

The documentation is licensed under the same terms as the DaemonEye project.
