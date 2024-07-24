# Use a specific Node.js version as a base image
FROM node:18.16.0

# Enable corepack
RUN corepack enable

# Set working directory
WORKDIR /app

# Copy package.json and pnpm-lock.yaml
COPY package.json pnpm-lock.yaml ./

# Install pnpm and project dependencies
RUN corepack prepare pnpm@latest --activate && pnpm install

# Copy the rest of the application code
COPY . .

# Build the TypeScript code
RUN pnpm run build

# Expose port 3000
EXPOSE 3000

# Command to run the application
CMD ["node", "dist/index.js"]
